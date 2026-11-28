//! GTA VI models: drawables, fragments and drawable dictionaries.
//!
//! A model resource has two parts: the *system* part with the structures,
//! and the *graphics* part with vertex and index data. Pointers are 64-bit
//! addresses, from `0x5000_0000` into the system part and from
//! `0x6000_0000` into the graphics part. All of it is little-endian.
//!
//! The resource starts with one of three roots, each with zero in its first
//! eight bytes and at 8 a pointer to the page counts:
//! - a **drawable**: shader group (16), radius (44), bounds (48, 64), four
//!   levels of detail (80 to 104), name (168);
//! - a **fragment**: its drawable (24) and name (168);
//! - a **drawable dictionary**: name hashes (32) and drawables (48), with
//!   their number at 40, 42, 56 and 58.
//!
//! A level of detail lists *models* (pointers, count at 8); a model lists
//! *geometries* (pointers at 8, count at 16) with their bounds (24) and
//! shaders (32, a 16-bit index each) and the number of bones it was made for
//! (20). A geometry has a vertex buffer (8) and, through a descriptor (64), an
//! index buffer.
//!
//! A buffer says how many elements of how many bytes it has (8, 12) and
//! where they are in the graphics part (24). The vertex buffer's format (64)
//! has, for each of the vertex's parts: its offset in the buffer (4 bytes
//! each from 0), its size (a byte each from `0xD0`) and its encoding (a byte
//! each from `0x104`); the vertex count is in the word at `0x138`, from bit
//! 10 on. So a buffer holds all positions, then all texture coordinates, …
//!
//! - Positions (part 0): three floats, or 64 bits of fixed point (24, 24
//!   and 16 bits) with a bias and a scale in the buffer's first bytes and an
//!   order of the axes.
//! - Skinning: up to eight weights of 8 bits (parts 16 and 17) with the
//!   numbers of their bones (20 and 21, 8 bits each, or 32 bits of which the
//!   lower 12 are the number), or one bone without a weight. Of the weights
//!   the four largest are kept. A bone's number is its place in the model's
//!   list of the hashes of its bones' names (40).
//! - Texture coordinates (parts 28 and 29): two floats, or two 16-bit numbers
//!   with a scale.
//! - What shades a vertex (part 24): how much of the sky it sees (red) and how
//!   much of what is put on the base shows (alpha).
//!
//! The index buffer is cut into *clusters* of up to 128 vertices and 256
//! triangles: a list of the vertices each cluster uses, and triangles as
//! three 7-bit numbers into its cluster's list.
//!
//! A shader names its textures by keys into the texture pools
//! ([`crate::ttpf`]); of those, the one for the base colour is read.
//!
//! A drawable's skeleton (24) has its bones at 40 and their number at 108. A
//! bone has 80 bytes: rotation, translation (16), the hash of its name (28),
//! scale (32), its name (48), the next sibling and the parent (68, 70, 16 bits
//! each, -1 for none), its number (72) and its tag (74).
//!
//! Parts of people and animals use bones of a skeleton in another file
//! ([`Mesh::bone_hashes`]).
//!
//! Not read: the other textures of materials, the lower levels of detail, and
//! which bone meshes without bone numbers belong to.
//!
//! The layout is the one found by the rpf9-explorer project
//! (<https://github.com/breadbbbb/rpf9-explorer>), whose decoder was read to
//! learn it; this reader is written anew.

use rage_core::bytes::LeRead;

use crate::drawable::{Bone, Drawable, Mesh, Shader, Skeleton};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    #[error("not a GTA VI model")]
    NotAModel,
    /// The graphics part ends before the data a buffer points to: at least
    /// this many bytes are needed.
    #[error("the model's vertex or index data is missing ({0} bytes are needed)")]
    Graphics(usize),
    #[error("damaged or unknown model data: {0}")]
    Bad(String),
}

fn bad(msg: impl Into<String>) -> ModelError {
    ModelError::Bad(msg.into())
}

const SYSTEM: u64 = 0x5000_0000;
const GRAPHICS: u64 = 0x6000_0000;
/// No part of a resource is larger than this.
const MAX_PART: u64 = 256 << 20;
const MAX_VERTICES: usize = 1_000_000;
const MAX_MESHES: usize = 1024;
/// Most models a level of detail lists.
const MAX_MODELS: usize = 256;
/// Slots of the vertex format.
const POSITION: usize = 0;
const WEIGHTS: [usize; 2] = [16, 17];
const BONES: [usize; 2] = [20, 21];
const TEXCOORD: usize = 28;
const COLOUR: usize = 24;
/// Most bones a skeleton has.
const MAX_BONES: usize = 4096;

/// The parameters of shaders that hold the base colour, the best first:
/// `diffusetex`, `diffusetexture`, and the first layer of the layered ones
/// (`lyr0diffusetex`, `diffusetexture_layer0`). Other textures of a shader
/// (normals, specular, masks) are not colours and must not stand in.
const BASE_COLOUR: [u32; 4] = [0xC8E8_C282, 0x2D14_F172, 0x3463_94EA, 0xCE7B_968E];

/// The name a texture of the pools goes by in a model: its key.
pub fn texture_name(key: u64) -> String {
    format!("{key:016x}")
}

/// What a resource starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Drawable,
    Fragment,
    Dictionary,
}

fn system_pointer(d: &[u8], at: usize) -> Option<usize> {
    let p = d.u64_at(at)?;
    (SYSTEM..SYSTEM + MAX_PART).contains(&p).then(|| (p - SYSTEM) as usize)
}

fn vec3(d: &[u8], at: usize) -> Option<[f32; 3]> {
    let f = |o| d.u32_at(at + o).map(f32::from_bits).filter(|v: &f32| v.is_finite() && v.abs() < 1e7);
    Some([f(0)?, f(4)?, f(8)?])
}

/// Whether a drawable starts at `at`: bounds that make sense, and pointers
/// where it has them. One inside another resource has no page counts.
fn is_drawable(d: &[u8], at: usize, inside: bool) -> bool {
    let check = || -> Option<bool> {
        let (min, max) = (vec3(d, at + 48)?, vec3(d, at + 64)?);
        let radius = f32::from_bits(d.u32_at(at + 44)?);
        let pages = if inside { d.u64_at(at + 8)? == 0 } else { system_pointer(d, at + 8).is_some() };
        let name = system_pointer(d, at + 168).is_some() || (inside && d.u64_at(at + 168)? == 0);
        Some(
            d.u64_at(at)? == 0
                && pages
                && name
                && system_pointer(d, at + 16).is_some()
                && system_pointer(d, at + 80).is_some()
                && (0..3).all(|i| min[i] <= max[i])
                && (0.0..1e6).contains(&radius),
        )
    };
    check().unwrap_or(false)
}

/// Whether the pointers and counts of a fragment are there; `None` if the bytes end before the counts.
fn has_fragment_fields(system: &[u8]) -> Option<bool> {
    let optional = |at| system.u64_at(at) == Some(0) || system_pointer(system, at).is_some();
    let counts = |at| system.u16_at(at).filter(|n| *n <= 4096);
    Some(
        system_pointer(system, 24).is_some()
            && system_pointer(system, 168).is_some()
            && [16, 32, 40].into_iter().all(optional)
            && counts(48)? > 0
            && counts(50).is_some()
            && counts(52)? <= counts(54)?,
    )
}

/// Whether the root is a drawable dictionary; `None` if the bytes end before its count.
fn is_dictionary(system: &[u8]) -> Option<bool> {
    let count = system.u16_at(56)?;
    let first = || is_drawable(system, system_pointer(system, system_pointer(system, 48)?)?, true).then_some(());
    Some(
        count > 0
            && [40, 42, 58].into_iter().all(|at| system.u16_at(at) == Some(count))
            && system_pointer(system, 32).is_some()
            && first().is_some(),
    )
}

/// What the resource whose system part starts with these bytes is; `None`
/// if it is no model, or if the bytes end before the drawable of a fragment
/// or the first one of a dictionary.
pub fn root(system: &[u8]) -> Option<Root> {
    if system.u64_at(0)? != 0 || system_pointer(system, 8).is_none() {
        return None;
    }
    if is_drawable(system, 0, false) {
        return Some(Root::Drawable);
    }
    if has_fragment_fields(system)? {
        // Other resources have pointers and small numbers there too: what
        // tells a fragment is the drawable it points to.
        return is_drawable(system, system_pointer(system, 24)?, true).then_some(Root::Fragment);
    }
    is_dictionary(system)?.then_some(Root::Dictionary)
}

/// The name of the model whose system part starts with these bytes: a
/// drawable's or a fragment's own name. `None` for anything else, for a
/// dictionary (its drawables have names, it has none), and when the name
/// lies after these bytes.
pub fn name(system: &[u8]) -> Option<String> {
    let text = |pointer_at: usize| -> Option<String> {
        let bytes = system.get(system_pointer(system, pointer_at)?..)?;
        let bytes = &bytes[..bytes.len().min(256)];
        let name = std::str::from_utf8(&bytes[..bytes.iter().position(|b| *b == 0)?]).ok()?;
        (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.to_owned())
    };
    match root(system)? {
        Root::Drawable => text(168),
        // The drawable inside is named like the fragment, with `.#ft`.
        Root::Fragment => {
            system_pointer(system, 24).and_then(|at| text(at + 168)).or_else(|| text(168).map(|n| format!("{n}.#ft")))
        }
        Root::Dictionary => None,
    }
}

/// How the bones of a model's meshes are numbered.
struct ModelBones {
    /// The hashes of the names of the model's bones; empty without such a list.
    hashes: Vec<u32>,
    /// The number of bones a mesh's bone numbers must stay below.
    count: usize,
    /// For each bone of the model, the skeleton's bone with that name, if the skeleton has them all.
    own: Option<Vec<u16>>,
}

impl ModelBones {
    /// Numbers the bones of `mesh` by the skeleton's, or leaves it the hashes to find them by.
    fn assign(&self, mesh: &mut Mesh) {
        match &self.own {
            Some(map) => mesh.bone_indices.iter_mut().flatten().for_each(|b| *b = map[usize::from(*b)]),
            None if !mesh.bone_indices.is_empty() => mesh.bone_hashes = self.hashes.clone(),
            None => {}
        }
    }
}

struct Reader<'a> {
    sys: &'a [u8],
    gfx: &'a [u8],
}

impl<'a> Reader<'a> {
    /// The system offset a pointer at `at` leads to, with `size` bytes there.
    fn s(&self, at: usize, size: usize) -> Result<usize, ModelError> {
        let to = system_pointer(self.sys, at).ok_or_else(|| bad("a pointer that leads nowhere"))?;
        if to + size > self.sys.len() { Err(bad("a structure outside the resource")) } else { Ok(to) }
    }

    /// The bytes of the graphics part a pointer at `at` leads to.
    fn g(&self, at: usize, size: usize) -> Result<&'a [u8], ModelError> {
        let p = self.sys.u64_at(at).filter(|p| (GRAPHICS..GRAPHICS + MAX_PART).contains(p));
        let to = (p.ok_or_else(|| bad("a buffer that leads nowhere"))? - GRAPHICS) as usize;
        self.gfx.get(to..to + size).ok_or(ModelError::Graphics(to + size))
    }

    fn u16(&self, at: usize) -> Result<usize, ModelError> {
        self.sys.u16_at(at).map(usize::from).ok_or_else(|| bad("truncated"))
    }

    fn u32(&self, at: usize) -> Result<u32, ModelError> {
        self.sys.u32_at(at).ok_or_else(|| bad("truncated"))
    }

    fn name(&self, at: usize) -> Result<String, ModelError> {
        let from = self.s(at, 1)?;
        let bytes = &self.sys[from..self.sys.len().min(from + 256)];
        let len = bytes.iter().position(|b| *b == 0).ok_or_else(|| bad("a name without an end"))?;
        Ok(String::from_utf8_lossy(&bytes[..len]).into_owned())
    }

    /// Where a buffer's data is: element count × element size bytes.
    fn buffer(&self, at: usize) -> Result<(&'a [u8], usize), ModelError> {
        let (count, stride) = (self.u32(at + 8)? as usize, self.u16(at + 12)?);
        let len = count.checked_mul(stride).filter(|n| (1..=MAX_PART as usize).contains(n));
        Ok((self.g(at + 24, len.ok_or_else(|| bad("a buffer of an impossible size"))?)?, stride))
    }

    /// The list of the most detailed level's models and their number.
    fn models(&self, at: usize) -> Result<(usize, usize), ModelError> {
        let lod = self.s(at + 80, 16)?;
        let models = self.u16(lod + 8)?;
        if !(1..=MAX_MODELS).contains(&models) {
            return Err(bad("an impossible number of models"));
        }
        Ok((self.s(lod, models * 8)?, models))
    }

    /// What a model's bones are called and which of the skeleton's they are.
    fn model_bones(&self, model: usize, skeleton: Option<&Skeleton>) -> Result<ModelBones, ModelError> {
        let used = self.u16(model + 20)?;
        // What its bones are called; a model without such a list has the skeleton's.
        let hashes: Vec<u32> = match self.s(model + 40, used * 4) {
            Ok(list) => (0..used).filter_map(|i| self.sys.u32_at(list + i * 4)).collect(),
            Err(_) => Vec::new(),
        };
        let skeleton_bones = skeleton.map_or(0, |s| s.bones.len());
        let count = if !hashes.is_empty() || used == skeleton_bones { used } else { 0 };
        // The skeleton's, if it has them all; else those of another one.
        let own: Option<Vec<u16>> = skeleton
            .filter(|_| !hashes.is_empty())
            .and_then(|s| hashes.iter().map(|&h| s.find_hash(h).map(|b| b as u16)).collect());
        Ok(ModelBones { hashes, count, own })
    }

    /// The geometries of the model at `model`, added to `meshes`.
    fn model(&self, model: usize, skeleton: Option<&Skeleton>, meshes: &mut Vec<Mesh>) -> Result<(), ModelError> {
        let count = self.u16(model + 16)?;
        if !(1..=MAX_MESHES).contains(&count) || meshes.len() + count > MAX_MESHES {
            return Err(bad("an impossible number of geometries"));
        }
        let geometries = self.s(model + 8, count * 8)?;
        let shaders = self.s(model + 32, count * 2).ok();
        let bones = self.model_bones(model, skeleton)?;
        for g in 0..count {
            let mut mesh = self.mesh(self.s(geometries + g * 8, 160)?, bones.count)?;
            bones.assign(&mut mesh);
            mesh.shader = shaders.and_then(|s| self.sys.u16_at(s + g * 2)).unwrap_or(0);
            meshes.push(mesh);
        }
        Ok(())
    }

    fn drawable(&self, at: usize, fallback_name: &str) -> Result<Drawable, ModelError> {
        if at + 176 > self.sys.len() {
            return Err(bad("a drawable outside the resource"));
        }
        let name = if self.sys.u64_at(at + 168) == Some(0) { fallback_name.to_owned() } else { self.name(at + 168)? };
        let bounds = |o| vec3(self.sys, at + o).ok_or_else(|| bad("bounds that are no numbers"));
        // The most detailed level.
        let (list, models) = self.models(at)?;
        let skeleton = self.skeleton(at);
        let mut meshes = Vec::new();
        for m in 0..models {
            self.model(self.s(list + m * 8, 56)?, skeleton.as_ref(), &mut meshes)?;
        }
        Ok(Drawable {
            name,
            meshes,
            shaders: self.shaders(at).unwrap_or_default(),
            textures: None,
            bounds_min: bounds(48)?,
            bounds_max: bounds(64)?,
            skeleton,
        })
    }

    /// A drawable's skeleton; `None` if it has none or it does not make sense.
    fn skeleton(&self, drawable: usize) -> Option<Skeleton> {
        let at = self.s(drawable + 24, 112).ok()?;
        let count = self.u16(at + 108).ok().filter(|n| (1..=MAX_BONES).contains(n))?;
        let list = self.s(at + 40, count * 80).ok()?;
        let bones = (0..count)
            .map(|i| {
                let bone = list + i * 80;
                let parent = self.sys.u16_at(bone + 70)? as i16;
                let turn = vec3(self.sys, bone)?;
                let w = f32::from_bits(self.sys.u32_at(bone + 12)?);
                let length = turn.iter().map(|v| v * v).sum::<f32>() + w * w;
                // Parents come first; a rotation is a unit quaternion.
                if i32::from(parent) >= i as i32 || parent < -1 || !(0.99..1.01).contains(&length) {
                    return None;
                }
                Some(Bone {
                    name: self.name(bone + 48).ok()?,
                    tag: self.sys.u16_at(bone + 74)?,
                    parent,
                    translation: vec3(self.sys, bone + 16)?,
                    rotation: [turn[0], turn[1], turn[2], w],
                    scale: vec3(self.sys, bone + 32)?,
                })
            })
            .collect::<Option<Vec<Bone>>>()?;
        Some(Skeleton { bones })
    }

    /// The key of the texture a shader binds to one of the parameters for
    /// the base colour. A shader (64 bytes) has its textures at 16 and, at
    /// 32, counts and its parameters: 8 bytes each from 8 on, the name hash
    /// and a word whose low two bits are 0 for a texture, with its number in
    /// the next eight. A texture (80 bytes) points at 48 to its asset, whose
    /// second word is the key by which the texture pools know it.
    fn bound_textures(&self, shader: usize) -> Option<Vec<(u32, u64)>> {
        let info = self.s(shader + 32, 8).ok()?;
        let (textures, parameters) = (usize::from(self.sys[info + 1]), usize::from(self.sys[info + 4]));
        let list = self.s(shader + 16, textures * 8).ok()?;
        let key = |parameter: usize| -> Option<(u32, u64)> {
            let at = info + 8 + parameter * 8;
            let (name, data) = (self.sys.u32_at(at)?, self.sys.u32_at(at + 4)?);
            let number = (data >> 2 & 255) as usize;
            if data & 3 != 0 || number >= textures {
                return None;
            }
            let texture = self.s(list + number * 8, 80).ok()?;
            let asset = self.s(texture + 48, 16).ok()?;
            let key = self.sys.u64_at(asset + 8).filter(|k| *k != 0)?;
            // In front of an asset: its kind, 6 for a texture of the pools.
            (self.sys.u32_at(asset.checked_sub(16)?)? == 6).then_some((name, key))
        };
        Some((0..parameters).filter_map(key).collect())
    }

    /// The shaders of a drawable, by their name hashes, each with the
    /// texture of its base colour: named by its key in the texture pools
    /// (16 hexadecimal digits).
    fn shaders(&self, drawable: usize) -> Result<Vec<Shader>, ModelError> {
        let group = self.s(drawable + 16, 64)?;
        let count = self.u16(group + 24)?;
        if count == 0 || count > 512 {
            return Ok(Vec::new());
        }
        let list = self.s(group + 16, count * 8)?;
        (0..count)
            .map(|i| {
                let shader = self.s(list + i * 8, 64)?;
                let bound = self.bound_textures(shader).unwrap_or_default();
                let base = BASE_COLOUR.iter().find_map(|wanted| bound.iter().find(|(name, _)| name == wanted));
                // The base colour first, under the name it has in other games.
                let mut samplers: Vec<(u32, String)> = base
                    .map(|(_, key)| (crate::drawable::param::DIFFUSE_TEX, texture_name(*key)))
                    .into_iter()
                    .collect();
                let others = bound.iter().filter(|b| Some(*b) != base);
                samplers.extend(others.map(|(name, key)| (*name, texture_name(*key))));
                let textures = base.map(|(_, key)| texture_name(*key)).into_iter().collect();
                Ok(Shader { name_hash: self.u32(shader)?, textures, samplers })
            })
            .collect()
    }

    /// The triangle indices of a geometry with `vertices` vertices.
    fn indices(&self, geometry: usize, vertices: usize) -> Result<Vec<u32>, ModelError> {
        let descriptor = self.s(geometry + 64, 16)?;
        let (index, stride) = self.buffer(self.s(descriptor, 64)?)?;
        if stride != 4 {
            return Err(bad("an index buffer of an unknown kind"));
        }
        clusters(index, vertices)
    }

    /// A geometry of a drawable with `bones` bones.
    fn mesh(&self, geometry: usize, bones: usize) -> Result<Mesh, ModelError> {
        let vb = self.s(geometry + 8, 80)?;
        let format = self.s(vb + 64, 320)?;
        let flags = self.u32(format + 0x138)?;
        let vertices = (flags >> 10) as usize;
        if flags & 1 == 0 || !(1..=MAX_VERTICES).contains(&vertices) {
            return Err(bad("an impossible vertex count"));
        }
        let (data, _) = self.buffer(vb)?;
        let vertex_data = VertexData { sys: self.sys, format, data, count: vertices };

        let positions = vertex_data.positions()?;
        let (uvs, uvs1) = (vertex_data.coordinates(TEXCOORD), vertex_data.coordinates(TEXCOORD + 1));
        let (bone_indices, bone_weights) = vertex_data.skinning(bones);
        let shading = vertex_data.shading();

        let indices = self.indices(geometry, vertices)?;
        let normals = smooth_normals(&positions, &indices);
        Ok(Mesh { positions, normals, uvs, uvs1, shading, indices, bone_indices, bone_weights, ..Default::default() })
    }
}

/// The vertices of a geometry: its vertex buffer and where the format (in the system part) puts each part.
struct VertexData<'a> {
    sys: &'a [u8],
    format: usize,
    data: &'a [u8],
    count: usize,
}

impl<'a> VertexData<'a> {
    /// A part's offset in the buffer, size and encoding.
    fn part(&self, slot: usize) -> (usize, u8, u8) {
        let offset = self.sys.u32_at(self.format + slot * 4).unwrap_or(0) as usize;
        (offset, self.sys[self.format + 0xD0 + slot], self.sys[self.format + 0x104 + slot])
    }

    /// `offset` if all vertices' elements of `size` bytes from there lie inside the buffer.
    fn within(&self, offset: usize, size: usize) -> Option<usize> {
        offset.checked_add(self.count * size).filter(|end| *end <= self.data.len()).map(|_| offset)
    }

    fn positions(&self) -> Result<Vec<[f32; 3]>, ModelError> {
        let (offset, size, encoding) = self.part(POSITION);
        let at = self.within(offset, usize::from(size)).ok_or_else(|| bad("positions outside their buffer"))?;
        let positions = match (size, encoding) {
            (12, 6) => (0..self.count).map(|i| vec3(self.data, at + i * 12)).collect::<Option<_>>(),
            (8, 16) => self.fixed_point_positions(at),
            _ => return Err(bad(format!("positions of an unknown kind ({size} bytes, encoding {encoding})"))),
        };
        positions.ok_or_else(|| bad("positions that are no numbers"))
    }

    /// Fixed point: 24 bits, 24 bits (the lowest byte and two more
    /// above the first number) and 16 bits.
    fn fixed_point_positions(&self, at: usize) -> Option<Vec<[f32; 3]>> {
        let data = self.data;
        let (bias, scale, axes) = (vec3(data, 96)?, vec3(data, 112)?, data.u32_at(108)?);
        let numbers = data[at..at + self.count * 8].as_chunks::<8>().0;
        Some(
            numbers
                .iter()
                .map(|c| {
                    let q = u64::from_le_bytes(*c);
                    let parts = [q >> 8 & 0xFF_FFFF, (q >> 32 & 0xFFFF) << 8 | q & 0xFF, q >> 48];
                    let p: [f32; 3] =
                        std::array::from_fn(|k| (f64::from(bias[k]) + f64::from(scale[k]) * parts[k] as f64) as f32);
                    match axes {
                        0 => [p[2], p[1], p[0]],
                        1 => [p[1], p[2], p[0]],
                        _ => p,
                    }
                })
                .collect(),
        )
    }

    /// The texture coordinates of a slot, empty when it has none or of an unknown kind.
    fn coordinates(&self, slot: usize) -> Vec<[f32; 2]> {
        let (offset, size, encoding) = self.part(slot);
        match (size, encoding, self.within(offset, usize::from(size))) {
            (8, 16, Some(at)) => self.float_coordinates(at),
            (4, 38, Some(at)) => self.scaled_coordinates(at),
            _ => Vec::new(),
        }
    }

    fn float_coordinates(&self, at: usize) -> Vec<[f32; 2]> {
        self.data[at..at + self.count * 8]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| [f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]])])
            .collect()
    }

    fn scaled_coordinates(&self, at: usize) -> Vec<[f32; 2]> {
        let scale = self.data.u32_at(124).map(f32::from_bits).filter(|s| s.is_finite()).unwrap_or(1.0);
        let number = |a: u8, b: u8| (f32::from(i16::from_le_bytes([a, b])) / 32767.0).max(-1.0) * scale;
        self.data[at..at + self.count * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| [number(c[0], c[1]), number(c[2], c[3])])
            .collect()
    }

    /// The bytes of a slot if it has exactly this element size and encoding.
    fn bytes(&self, slot: usize, size: usize, encoding: u8) -> Option<&'a [u8]> {
        let (offset, own_size, own_encoding) = self.part(slot);
        let at = self.within(offset, size).filter(|_| (usize::from(own_size), own_encoding) == (size, encoding))?;
        self.data.get(at..at + self.count * size)
    }

    /// Bone numbers and weights of every vertex, with bones the skeleton (of `bones` bones) has, or none.
    fn skinning(&self, bones: usize) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
        (0..self.count)
            .map(|i| self.skin_of(i).filter(|(n, _)| n.iter().all(|n| usize::from(*n) < bones)))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
            .into_iter()
            .unzip()
    }

    fn skin_of(&self, i: usize) -> Option<([u16; 4], [f32; 4])> {
        if let Some(single) = self.bytes(BONES[0], 1, 62) {
            return Some(([u16::from(single[i]), 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
        }
        let mut found = self.weighted_bones(i);
        found.sort_by_key(|(w, _)| std::cmp::Reverse(*w));
        let (mut numbers, mut weights) = ([0; 4], [0.0; 4]);
        for (k, (w, n)) in found.into_iter().filter(|(w, _)| *w > 0).take(4).enumerate() {
            numbers[k] = n;
            weights[k] = f32::from(w) / 255.0;
        }
        let total: f32 = weights.iter().sum();
        (total > 0.0).then(|| (numbers, weights.map(|w| w / total)))
    }

    /// The (weight, bone number) pairs of vertex `i`, from both weight parts.
    fn weighted_bones(&self, i: usize) -> Vec<(u8, u16)> {
        let mut found: Vec<(u8, u16)> = Vec::with_capacity(8);
        for (weights, numbers) in WEIGHTS.into_iter().zip(BONES) {
            let Some(w) = self.bytes(weights, 4, 28) else { continue };
            if let Some(n) = self.bytes(numbers, 4, 30) {
                found.extend((0..4).map(|k| (w[i * 4 + k], u16::from(n[i * 4 + k]))));
            } else if let Some(n) = self.bytes(numbers, 16, 3) {
                // The bone is in the lower 12 bits.
                let number = |k: usize| u16::from_le_bytes([n[(i * 4 + k) * 4], n[(i * 4 + k) * 4 + 1]]) & 0xFFF;
                found.extend((0..4).map(|k| (w[i * 4 + k], number(k))));
            }
        }
        found
    }

    fn shading(&self) -> Vec<[u8; 4]> {
        self.bytes(COLOUR, 4, 28).map(|all| all.as_chunks::<4>().0.to_vec()).unwrap_or_default()
    }
}

/// The flag of an index buffer with 16-bit vertex references.
const REFS_16_BIT: u32 = 0x8000_0000;

fn index_word(index: &[u8], at: usize) -> Result<u32, ModelError> {
    index.u32_at(at).ok_or_else(|| bad("an index buffer that is cut off"))
}

/// The cut-up index buffer: where its clusters, triangles and vertex references are.
struct ClusterBuffer<'a> {
    index: &'a [u8],
    vertices: usize,
    cluster_count: usize,
    triangles: usize,
    refs: usize,
    wide: bool,
    triangles_at: usize,
    refs_at: usize,
}

impl<'a> ClusterBuffer<'a> {
    fn parse(index: &'a [u8], vertices: usize) -> Result<Self, ModelError> {
        let flags = index_word(index, 0)?;
        let cluster_count = (flags & if flags & 0x4000_0000 != 0 { 0xFFFF } else { 0x7FFF_FFFF }) as usize;
        let (triangles, refs) = (index_word(index, 4)? as usize, index_word(index, 8)? as usize);
        if !(1..=2048).contains(&cluster_count) || !(1..=65535).contains(&triangles) || !(1..=65535).contains(&refs) {
            return Err(bad("impossible counts in an index buffer"));
        }
        let wide = flags & REFS_16_BIT == 0;
        let triangles_at = 12 + cluster_count * 16;
        let refs_at = triangles_at + triangles * 4;
        Ok(Self { index, vertices, cluster_count, triangles, refs, wide, triangles_at, refs_at })
    }

    fn word(&self, at: usize) -> Result<u32, ModelError> {
        index_word(self.index, at)
    }

    /// The vertex the `i`th reference names.
    fn reference(&self, i: usize) -> Result<u32, ModelError> {
        let v = if self.wide {
            self.word(self.refs_at + i * 4)?
        } else {
            u32::from(self.index.u16_at(self.refs_at + i * 2).ok_or_else(|| bad("truncated"))?)
        };
        if (v as usize) < self.vertices { Ok(v) } else { Err(bad("a triangle with a vertex that does not exist")) }
    }

    /// The corners of the triangle with word `w`, whose cluster has `own_refs` references from `first_ref` on.
    fn read_triangle(&self, w: u32, first_ref: usize, own_refs: usize, out: &mut Vec<u32>) -> Result<(), ModelError> {
        for corner in 0..3 {
            let local = (w >> (corner * 7) & 127) as usize;
            if local >= own_refs {
                return Err(bad("a triangle outside its cluster"));
            }
            out.push(self.reference(first_ref + local)?);
        }
        Ok(())
    }

    fn read_cluster(&self, cluster: usize, out: &mut Vec<u32>) -> Result<(), ModelError> {
        let (counts, starts) = (self.word(12 + cluster * 16)?, self.word(16 + cluster * 16)?);
        let (own_refs, own_triangles) = ((counts & 127) as usize + 1, (counts >> 7 & 255) as usize + 1);
        let (first_ref, first_triangle) = ((starts & 0xFFFF) as usize, (starts >> 16) as usize);
        if first_ref + own_refs > self.refs || first_triangle + own_triangles > self.triangles {
            return Err(bad("a cluster outside its buffer"));
        }
        for t in first_triangle..first_triangle + own_triangles {
            self.read_triangle(self.word(self.triangles_at + t * 4)?, first_ref, own_refs, out)?;
        }
        Ok(())
    }
}

/// The triangles of an index buffer: a word of flags and the cluster count,
/// the triangle count, the count of vertex references; 16 bytes per cluster
/// (vertices and triangles minus one in 7 and 8 bits, then where its
/// references and triangles start); a word per triangle (three 7-bit numbers
/// into the cluster's references, the cluster from bit 21); the references
/// (16 bits each if the top flag is set, else 32).
fn clusters(index: &[u8], vertices: usize) -> Result<Vec<u32>, ModelError> {
    let buffer = ClusterBuffer::parse(index, vertices)?;
    let mut out = Vec::with_capacity(buffer.triangles * 3);
    for cluster in 0..buffer.cluster_count {
        buffer.read_cluster(cluster, &mut out)?;
    }
    if out.len() != buffer.triangles * 3 {
        return Err(bad("clusters that do not add up to the triangle count"));
    }
    Ok(out)
}

/// A normal per vertex: the sum of the normals of its triangles.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0f32; 3]; positions.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for i in t {
            for k in 0..3 {
                normals[*i as usize][k] += n[k];
            }
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        *n = if len > 0.0 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 0.0, 1.0] };
    }
    normals
}

/// Reads the drawables of a resource: one, or those of a dictionary.
/// [`ModelError::Graphics`] says the graphics part is too short.
pub fn parse(system: &[u8], graphics: &[u8]) -> Result<Vec<Drawable>, ModelError> {
    let reader = Reader { sys: system, gfx: graphics };
    reader.check_pages()?;
    match root(system).ok_or(ModelError::NotAModel)? {
        Root::Drawable => Ok(vec![reader.drawable(0, "")?]),
        Root::Fragment => reader.fragment(),
        Root::Dictionary => reader.dictionary(),
    }
}

impl Reader<'_> {
    /// Whether the page counts a model's resource starts with are plausible.
    fn check_pages(&self) -> Result<(), ModelError> {
        let pages = self.s(8, 16).map_err(|_| ModelError::NotAModel)?;
        if !(1..=32).contains(&self.sys[pages + 8]) || !(1..=32).contains(&self.sys[pages + 9]) {
            return Err(ModelError::NotAModel);
        }
        Ok(())
    }

    /// The drawable of a fragment.
    fn fragment(&self) -> Result<Vec<Drawable>, ModelError> {
        let at = self.s(24, 176)?;
        if !is_drawable(self.sys, at, true) {
            return Err(ModelError::NotAModel);
        }
        Ok(vec![self.drawable(at, &self.name(168)?)?])
    }

    /// The drawables of a dictionary; those that cannot be read cost only themselves.
    fn dictionary(&self) -> Result<Vec<Drawable>, ModelError> {
        let count = self.u16(56)?;
        let list = self.s(48, count * 8)?;
        let mut drawables = Vec::with_capacity(count);
        let mut first_error = None;
        for i in 0..count {
            let at = self.s(list + i * 8, 176)?;
            if !is_drawable(self.sys, at, true) {
                return Err(ModelError::NotAModel);
            }
            match self.drawable(at, &format!("drawable {i}")) {
                Ok(d) => drawables.push(d),
                // More graphics data may help; anything else only costs this one.
                Err(e @ ModelError::Graphics(_)) => return Err(e),
                Err(e) => first_error = first_error.or(Some(e)),
            }
        }
        if drawables.is_empty() { Err(first_error.unwrap_or(ModelError::NotAModel)) } else { Ok(drawables) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(d: &mut [u8], at: usize, bytes: &[u8]) {
        d[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn pointer(d: &mut [u8], at: usize, base: u64, to: usize) {
        put(d, at, &(base + to as u64).to_le_bytes());
    }

    /// A drawable with one model of one geometry: a square of two triangles
    /// in one cluster, positions as floats, texture coordinates as floats.
    fn resource() -> (Vec<u8>, Vec<u8>) {
        let mut s = vec![0u8; 0x800];
        let mut g = vec![0u8; 0x200];
        let (pages, group, lod, models, model, geometries, geometry) =
            (0x100, 0x120, 0x180, 0x1A0, 0x1B0, 0x200, 0x210);
        let (vb, format, descriptor, ib, name, shaders, shader) = (0x300, 0x380, 0x500, 0x520, 0x580, 0x5A0, 0x5B0);
        pointer(&mut s, 8, SYSTEM, pages);
        s[pages + 8] = 1;
        s[pages + 9] = 1;
        pointer(&mut s, 16, SYSTEM, group);
        put(&mut s, 44, &2.0f32.to_le_bytes());
        for (i, v) in [0.0f32, 0.0, 0.0].iter().chain(&[0.0]).chain(&[1.0, 1.0, 0.0]).enumerate() {
            put(&mut s, 48 + i * 4, &v.to_le_bytes());
        }
        pointer(&mut s, 80, SYSTEM, lod);
        pointer(&mut s, 168, SYSTEM, name);
        put(&mut s, name, b"square\0");
        // One shader.
        pointer(&mut s, group + 16, SYSTEM, shaders);
        put(&mut s, group + 24, &[1, 0, 1, 0]);
        pointer(&mut s, shaders, SYSTEM, shader);
        put(&mut s, shader, &0xABCD_1234u32.to_le_bytes());
        pointer(&mut s, lod, SYSTEM, models);
        put(&mut s, lod + 8, &[1, 0, 1, 0]);
        pointer(&mut s, models, SYSTEM, model);
        pointer(&mut s, model + 8, SYSTEM, geometries);
        put(&mut s, model + 16, &[1, 0, 1, 0]);
        pointer(&mut s, geometries, SYSTEM, geometry);
        pointer(&mut s, geometry + 8, SYSTEM, vb);
        pointer(&mut s, geometry + 64, SYSTEM, descriptor);
        // Vertex buffer: 4 vertices; positions at 0x80, texture coordinates at 0xB0.
        put(&mut s, vb + 8, &0x100u32.to_le_bytes());
        put(&mut s, vb + 12, &[1, 0]);
        pointer(&mut s, vb + 24, GRAPHICS, 0);
        pointer(&mut s, vb + 64, SYSTEM, format);
        put(&mut s, format, &0x80u32.to_le_bytes());
        s[format + 0xD0] = 12;
        s[format + 0x104] = 6;
        put(&mut s, format + TEXCOORD * 4, &0xB0u32.to_le_bytes());
        s[format + 0xD0 + TEXCOORD] = 8;
        s[format + 0x104 + TEXCOORD] = 16;
        put(&mut s, format + 0x138, &(4u32 << 10 | 1).to_le_bytes());
        let corners = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (i, c) in corners.iter().enumerate() {
            put(&mut g, 0x80 + i * 12, &c[0].to_le_bytes());
            put(&mut g, 0x84 + i * 12, &c[1].to_le_bytes());
            put(&mut g, 0xB0 + i * 8, &c[0].to_le_bytes());
            put(&mut g, 0xB4 + i * 8, &c[1].to_le_bytes());
        }
        // Index buffer at 0x100 of the graphics part: one cluster, two
        // triangles, four references of 16 bits.
        pointer(&mut s, descriptor, SYSTEM, ib);
        put(&mut s, ib + 8, &16u32.to_le_bytes());
        put(&mut s, ib + 12, &[4, 0]);
        pointer(&mut s, ib + 24, GRAPHICS, 0x100);
        let words = [0x8000_0001u32, 2, 4, (4 - 1) | (2 - 1) << 7, 0, 0, 0, 1 << 7 | 2 << 14, 2 << 7 | 3 << 14];
        for (i, w) in words.iter().enumerate() {
            put(&mut g, 0x100 + i * 4, &w.to_le_bytes());
        }
        for (i, r) in [0u16, 1, 2, 3].iter().enumerate() {
            put(&mut g, 0x100 + 36 + i * 2, &r.to_le_bytes());
        }
        (s, g)
    }

    #[test]
    fn reads_a_drawable() {
        let (s, g) = resource();
        assert_eq!(root(&s), Some(Root::Drawable));
        let drawables = parse(&s, &g).unwrap();
        let d = &drawables[0];
        assert_eq!((d.name.as_str(), d.meshes.len(), d.shaders[0].name_hash), ("square", 1, 0xABCD_1234));
        let mesh = &d.meshes[0];
        assert_eq!(mesh.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(mesh.uvs[2], [1.0, 1.0]);
        assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
        assert!(mesh.normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
        assert_eq!(d.bounds_max, [1.0, 1.0, 0.0]);
        assert_eq!(name(&s).as_deref(), Some("square"));
        assert_eq!(name(&s[..0x400]), None);
    }

    #[test]
    fn reads_the_skeleton_and_skinning() {
        let (mut s, mut g) = resource();
        s.resize(0xA00, 0);
        g.resize(0x300, 0);
        let (skeleton, bones, names) = (0x800, 0x880, 0x960);
        pointer(&mut s, 24, SYSTEM, skeleton);
        pointer(&mut s, skeleton + 40, SYSTEM, bones);
        put(&mut s, skeleton + 108, &[2]);
        for i in 0..2usize {
            let at = bones + i * 80;
            put(&mut s, at + 12, &1.0f32.to_le_bytes());
            put(&mut s, at + 16, &(i as f32).to_le_bytes());
            for k in 0..3 {
                put(&mut s, at + 32 + k * 4, &1.0f32.to_le_bytes());
            }
            pointer(&mut s, at + 48, SYSTEM, names + i * 8);
            put(&mut s, at + 70, &(i as i16 - 1).to_le_bytes());
            put(&mut s, at + 74, &[7 + i as u8]);
        }
        put(&mut s, names, b"root\0");
        put(&mut s, names + 8, b"tip\0");
        // Weights at 0x200 and bone numbers at 0x210 of the graphics part.
        put(&mut s, 0x1B0 + 20, &[2]);
        put(&mut s, 0x308, &0x300u32.to_le_bytes());
        let format = 0x380;
        for (slot, offset, encoding) in [(16, 0x200u32, 28), (20, 0x210, 30)] {
            put(&mut s, format + slot * 4, &offset.to_le_bytes());
            s[format + 0xD0 + slot] = 4;
            s[format + 0x104 + slot] = encoding;
        }
        for i in 0..4 {
            put(&mut g, 0x200 + i * 4, &[51, 204, 0, 0]);
            put(&mut g, 0x210 + i * 4, &[0, 1, 9, 9]);
        }
        let d = &parse(&s, &g).unwrap()[0];
        let skeleton = d.skeleton.as_ref().unwrap();
        let found: Vec<_> = skeleton.bones.iter().map(|b| (b.name.as_str(), b.parent, b.tag)).collect();
        assert_eq!(found, [("root", -1, 7), ("tip", 0, 8)]);
        assert_eq!(skeleton.bones[1].translation, [1.0, 0.0, 0.0]);
        assert_eq!(skeleton.find_hash(rage_core::hash::joaat("TIP")), Some(1));
        let mesh = &d.meshes[0];
        assert_eq!((mesh.bone_indices[3], mesh.bone_weights[3]), ([1, 0, 0, 0], [0.8, 0.2, 0.0, 0.0]));
        // A bone the skeleton does not have: not skinned at all.
        g[0x214] = 2;
        assert!(parse(&s, &g).unwrap()[0].meshes[0].bone_indices.is_empty());
        // A model for another number of bones.
        g[0x214] = 1;
        s[0x1B0 + 20] = 3;
        assert!(parse(&s, &g).unwrap()[0].meshes[0].bone_indices.is_empty());
        // A parent behind its child: no skeleton.
        s[bones + 70] = 1;
        assert!(parse(&s, &g).unwrap()[0].skeleton.is_none());
    }

    #[test]
    fn says_when_graphics_data_is_missing() {
        let (s, g) = resource();
        assert_eq!(parse(&s, &g[..0x80]).unwrap_err(), ModelError::Graphics(0x100));
        assert_eq!(parse(&s, &g[..0x120]).unwrap_err(), ModelError::Graphics(0x140));
        let mut other = s.clone();
        other[0] = 1;
        assert_eq!(root(&other), None);
        assert_eq!(parse(&vec![0u8; 0x400], &g).unwrap_err(), ModelError::NotAModel);
    }

    #[test]
    fn fixed_point_positions() {
        let (mut s, mut g) = resource();
        let format = 0x380;
        s[format + 0xD0] = 8;
        s[format + 0x104] = 16;
        // Bias (1, 2, 3), scale (0.5, 0.25, 2), axes as they are.
        for (i, v) in [1.0f32, 2.0, 3.0].iter().enumerate() {
            put(&mut g, 96 + i * 4, &v.to_le_bytes());
        }
        put(&mut g, 108, &2u32.to_le_bytes());
        for (i, v) in [0.5f32, 0.25, 2.0].iter().enumerate() {
            put(&mut g, 112 + i * 4, &v.to_le_bytes());
        }
        // x = 2, y = 0x000104 (two high bytes 0x0001, low byte 4), z = 3.
        let q: u64 = 4 | 2 << 8 | 1 << 32 | 3 << 48;
        for i in 0..4 {
            put(&mut g, 0x80 + i * 8, &q.to_le_bytes());
        }
        let mesh = &parse(&s, &g).unwrap()[0].meshes[0];
        assert_eq!(mesh.positions[0], [1.0 + 0.5 * 2.0, 2.0 + 0.25 * 260.0, 3.0 + 2.0 * 3.0]);
    }
}
