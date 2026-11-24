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
//!   numbers of their bones (20 and 21, 8 bits each), or one bone without a
//!   weight. Of the weights the four largest are kept.
//! - Texture coordinates (part 28): two floats, or two 16-bit numbers with a
//!   scale.
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
//! Not read: the other textures of materials, the lower levels of detail,
//! which bone meshes without bone numbers belong to, and the skinning of the
//! parts of people and animals (made for other bones than the part's own
//! skeleton has, some with bone numbers of 32 bits).
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
/// Slots of the vertex format.
const POSITION: usize = 0;
const WEIGHTS: [usize; 2] = [16, 17];
const BONES: [usize; 2] = [20, 21];
const TEXCOORD: usize = 28;
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
    let optional = |at| system.u64_at(at) == Some(0) || system_pointer(system, at).is_some();
    let counts = |at| system.u16_at(at).filter(|n| *n <= 4096);
    if system_pointer(system, 24).is_some()
        && system_pointer(system, 168).is_some()
        && [16, 32, 40].into_iter().all(optional)
        && counts(48)? > 0
        && counts(50).is_some()
        && counts(52)? <= counts(54)?
    {
        // Other resources have pointers and small numbers there too: what
        // tells a fragment is the drawable it points to.
        return is_drawable(system, system_pointer(system, 24)?, true).then_some(Root::Fragment);
    }
    let count = system.u16_at(56)?;
    let first = || is_drawable(system, system_pointer(system, system_pointer(system, 48)?)?, true).then_some(());
    (count > 0
        && [40, 42, 58].into_iter().all(|at| system.u16_at(at) == Some(count))
        && system_pointer(system, 32).is_some()
        && first().is_some())
    .then_some(Root::Dictionary)
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

    fn drawable(&self, at: usize, fallback_name: &str) -> Result<Drawable, ModelError> {
        if at + 176 > self.sys.len() {
            return Err(bad("a drawable outside the resource"));
        }
        let name = if self.sys.u64_at(at + 168) == Some(0) { fallback_name.to_owned() } else { self.name(at + 168)? };
        let bounds = |o| vec3(self.sys, at + o).ok_or_else(|| bad("bounds that are no numbers"));
        // The most detailed level.
        let lod = self.s(at + 80, 16)?;
        let models = self.u16(lod + 8)?;
        if !(1..=256).contains(&models) {
            return Err(bad("an impossible number of models"));
        }
        let list = self.s(lod, models * 8)?;
        let skeleton = self.skeleton(at);
        let bones = skeleton.as_ref().map_or(0, |s| s.bones.len());
        let mut meshes = Vec::new();
        for m in 0..models {
            let model = self.s(list + m * 8, 56)?;
            let count = self.u16(model + 16)?;
            if !(1..=MAX_MESHES).contains(&count) || meshes.len() + count > MAX_MESHES {
                return Err(bad("an impossible number of geometries"));
            }
            let geometries = self.s(model + 8, count * 8)?;
            let shaders = self.s(model + 32, count * 2).ok();
            // A model made for other bones than the skeleton's is not skinned.
            let bones = if self.u16(model + 20)? == bones { bones } else { 0 };
            for g in 0..count {
                let mut mesh = self.mesh(self.s(geometries + g * 8, 160)?, bones)?;
                mesh.shader = shaders.and_then(|s| self.sys.u16_at(s + g * 2)).unwrap_or(0);
                meshes.push(mesh);
            }
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
    fn base_texture(&self, shader: usize) -> Option<u64> {
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
        let bound: Vec<(u32, u64)> = (0..parameters).filter_map(key).collect();
        BASE_COLOUR.iter().find_map(|wanted| bound.iter().find(|(name, _)| name == wanted).map(|(_, key)| *key))
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
                let texture: Vec<String> = self.base_texture(shader).map(texture_name).into_iter().collect();
                let samplers = texture.iter().map(|name| (crate::drawable::param::DIFFUSE_TEX, name.clone())).collect();
                Ok(Shader { name_hash: self.u32(shader)?, textures: texture, samplers })
            })
            .collect()
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
        let part = |slot: usize| -> (usize, u8, u8) {
            let offset = self.sys.u32_at(format + slot * 4).unwrap_or(0) as usize;
            (offset, self.sys[format + 0xD0 + slot], self.sys[format + 0x104 + slot])
        };
        let within = |offset: usize, size: usize| {
            offset.checked_add(vertices * size).filter(|end| *end <= data.len()).map(|_| offset)
        };

        let (offset, size, encoding) = part(POSITION);
        let at = within(offset, usize::from(size)).ok_or_else(|| bad("positions outside their buffer"))?;
        let positions: Vec<[f32; 3]> = match (size, encoding) {
            (12, 6) => (0..vertices).map(|i| vec3(data, at + i * 12)).collect::<Option<_>>(),
            // Fixed point: 24 bits, 24 bits (the lowest byte and two more
            // above the first number) and 16 bits.
            (8, 16) => (|| {
                let (bias, scale, axes) = (vec3(data, 96)?, vec3(data, 112)?, data.u32_at(108)?);
                let numbers = data[at..at + vertices * 8].as_chunks::<8>().0;
                Some(
                    numbers
                        .iter()
                        .map(|c| {
                            let q = u64::from_le_bytes(*c);
                            let parts = [q >> 8 & 0xFF_FFFF, (q >> 32 & 0xFFFF) << 8 | q & 0xFF, q >> 48];
                            let p: [f32; 3] = std::array::from_fn(|k| {
                                (f64::from(bias[k]) + f64::from(scale[k]) * parts[k] as f64) as f32
                            });
                            match axes {
                                0 => [p[2], p[1], p[0]],
                                1 => [p[1], p[2], p[0]],
                                _ => p,
                            }
                        })
                        .collect(),
                )
            })(),
            _ => return Err(bad(format!("positions of an unknown kind ({size} bytes, encoding {encoding})"))),
        }
        .ok_or_else(|| bad("positions that are no numbers"))?;

        let (offset, size, encoding) = part(TEXCOORD);
        let uvs: Vec<[f32; 2]> = match (size, encoding, within(offset, usize::from(size))) {
            (8, 16, Some(at)) => data[at..at + vertices * 8]
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| [f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]])])
                .collect(),
            (4, 38, Some(at)) => {
                let scale = data.u32_at(124).map(f32::from_bits).filter(|s| s.is_finite()).unwrap_or(1.0);
                let number = |a: u8, b: u8| (f32::from(i16::from_le_bytes([a, b])) / 32767.0).max(-1.0) * scale;
                data[at..at + vertices * 4]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| [number(c[0], c[1]), number(c[2], c[3])])
                    .collect()
            }
            _ => Vec::new(),
        };

        let bytes = |slot: usize, size: usize, encoding: u8| -> Option<&[u8]> {
            let (offset, own_size, own_encoding) = part(slot);
            let at = within(offset, size).filter(|_| (usize::from(own_size), own_encoding) == (size, encoding))?;
            data.get(at..at + vertices * size)
        };
        let skinning = |i: usize| -> Option<([u16; 4], [f32; 4])> {
            if let Some(single) = bytes(BONES[0], 1, 62) {
                return Some(([u16::from(single[i]), 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
            }
            let mut found: Vec<(u8, u8)> = Vec::with_capacity(8);
            for (weights, numbers) in WEIGHTS.into_iter().zip(BONES) {
                if let (Some(w), Some(n)) = (bytes(weights, 4, 28), bytes(numbers, 4, 30)) {
                    found.extend((0..4).map(|k| (w[i * 4 + k], n[i * 4 + k])));
                }
            }
            found.sort_by_key(|(w, _)| std::cmp::Reverse(*w));
            let (mut numbers, mut weights) = ([0; 4], [0.0; 4]);
            for (k, (w, n)) in found.into_iter().filter(|(w, _)| *w > 0).take(4).enumerate() {
                numbers[k] = u16::from(n);
                weights[k] = f32::from(w) / 255.0;
            }
            let total: f32 = weights.iter().sum();
            (total > 0.0).then(|| (numbers, weights.map(|w| w / total)))
        };
        // All vertices, with bones the skeleton has, or none.
        let (bone_indices, bone_weights) = (0..vertices)
            .map(|i| skinning(i).filter(|(n, _)| n.iter().all(|n| usize::from(*n) < bones)))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
            .into_iter()
            .unzip();

        let descriptor = self.s(geometry + 64, 16)?;
        let (index, stride) = self.buffer(self.s(descriptor, 64)?)?;
        if stride != 4 {
            return Err(bad("an index buffer of an unknown kind"));
        }
        let indices = clusters(index, vertices)?;
        let normals = smooth_normals(&positions, &indices);
        Ok(Mesh { positions, normals, uvs, indices, bone_indices, bone_weights, ..Default::default() })
    }
}

/// The triangles of an index buffer: a word of flags and the cluster count,
/// the triangle count, the count of vertex references; 16 bytes per cluster
/// (vertices and triangles minus one in 7 and 8 bits, then where its
/// references and triangles start); a word per triangle (three 7-bit numbers
/// into the cluster's references, the cluster from bit 21); the references
/// (16 bits each if the top flag is set, else 32).
fn clusters(index: &[u8], vertices: usize) -> Result<Vec<u32>, ModelError> {
    let word = |at: usize| index.u32_at(at).ok_or_else(|| bad("an index buffer that is cut off"));
    let flags = word(0)?;
    let cluster_count = (flags & if flags & 0x4000_0000 != 0 { 0xFFFF } else { 0x7FFF_FFFF }) as usize;
    let (triangles, refs) = (word(4)? as usize, word(8)? as usize);
    if !(1..=2048).contains(&cluster_count) || !(1..=65535).contains(&triangles) || !(1..=65535).contains(&refs) {
        return Err(bad("impossible counts in an index buffer"));
    }
    let wide = flags & 0x8000_0000 == 0;
    let (triangles_at, refs_at) = (12 + cluster_count * 16, 12 + cluster_count * 16 + triangles * 4);
    let reference = |i: usize| -> Result<u32, ModelError> {
        let v = if wide {
            word(refs_at + i * 4)?
        } else {
            u32::from(index.u16_at(refs_at + i * 2).ok_or_else(|| bad("truncated"))?)
        };
        if (v as usize) < vertices { Ok(v) } else { Err(bad("a triangle with a vertex that does not exist")) }
    };
    let mut out = Vec::with_capacity(triangles * 3);
    for cluster in 0..cluster_count {
        let (counts, starts) = (word(12 + cluster * 16)?, word(16 + cluster * 16)?);
        let (own_refs, own_triangles) = ((counts & 127) as usize + 1, (counts >> 7 & 255) as usize + 1);
        let (first_ref, first_triangle) = ((starts & 0xFFFF) as usize, (starts >> 16) as usize);
        if first_ref + own_refs > refs || first_triangle + own_triangles > triangles {
            return Err(bad("a cluster outside its buffer"));
        }
        for t in first_triangle..first_triangle + own_triangles {
            let w = word(triangles_at + t * 4)?;
            for corner in 0..3 {
                let local = (w >> (corner * 7) & 127) as usize;
                if local >= own_refs {
                    return Err(bad("a triangle outside its cluster"));
                }
                out.push(reference(first_ref + local)?);
            }
        }
    }
    if out.len() != triangles * 3 {
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
    let pages = reader.s(8, 16).map_err(|_| ModelError::NotAModel)?;
    if !(1..=32).contains(&system[pages + 8]) || !(1..=32).contains(&system[pages + 9]) {
        return Err(ModelError::NotAModel);
    }
    match root(system).ok_or(ModelError::NotAModel)? {
        Root::Drawable => Ok(vec![reader.drawable(0, "")?]),
        Root::Fragment => {
            let at = reader.s(24, 176)?;
            if !is_drawable(system, at, true) {
                return Err(ModelError::NotAModel);
            }
            Ok(vec![reader.drawable(at, &reader.name(168)?)?])
        }
        Root::Dictionary => {
            let count = reader.u16(56)?;
            let list = reader.s(48, count * 8)?;
            let mut drawables = Vec::with_capacity(count);
            let mut first_error = None;
            for i in 0..count {
                let at = reader.s(list + i * 8, 176)?;
                if !is_drawable(system, at, true) {
                    return Err(ModelError::NotAModel);
                }
                match reader.drawable(at, &format!("drawable {i}")) {
                    Ok(d) => drawables.push(d),
                    // More graphics data may help; anything else only costs this one.
                    Err(e @ ModelError::Graphics(_)) => return Err(e),
                    Err(e) => first_error = first_error.or(Some(e)),
                }
            }
            if drawables.is_empty() { Err(first_error.unwrap_or(ModelError::NotAModel)) } else { Ok(drawables) }
        }
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
