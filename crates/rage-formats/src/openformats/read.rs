//! Reading GTA V models and texture dictionaries into the openFormats model.

use rage_resource::{Resource, ResourceError};

use super::*;
use crate::drawable::{Encoding, VERSION_YDR_LEGACY, VERSION_YFT_LEGACY, bad};
use crate::ytd;

const SYS: u64 = 0x5000_0000;

fn enhanced(res: &Resource) -> Result<bool, ResourceError> {
    use crate::drawable::{VERSION_YDR_ENHANCED, VERSION_YDR_ENHANCED_OLD, VERSION_YFT_ENHANCED};
    match res.version {
        VERSION_YDR_ENHANCED | VERSION_YFT_ENHANCED | VERSION_YDR_ENHANCED_OLD => Ok(true),
        VERSION_YDR_LEGACY | VERSION_YFT_LEGACY => Ok(false),
        v => Err(bad(format!("resource version {v} is not a GTA V model"))),
    }
}

/// A `.ydr` file.
pub fn read_ydr(res: &Resource, name: &str) -> Result<OfDrawable, ResourceError> {
    let mut d = read_drawable_at(res, SYS, enhanced(res)?)?;
    d.name = name.to_owned();
    Ok(d)
}

/// A `.yft` file: its main drawable and the parts that have models.
pub fn read_yft(res: &Resource, name: &str) -> Result<OfFragment, ResourceError> {
    let enhanced = enhanced(res)?;
    let main = res.u64(SYS + 0x30)?;
    if main == 0 {
        return Err(bad("the fragment has no drawable"));
    }
    let mut drawable = read_drawable_at(res, main, enhanced)?;
    drawable.name = name.to_owned();
    let mut out = OfFragment { drawable, children: Vec::new() };
    let group = res.u64(SYS + 0xF0)?;
    if group == 0 {
        return Ok(out);
    }
    let Some(lod) = (0..3).map(|i| res.u64(group + 0x10 + i * 8)).find_map(|p| p.ok().filter(|&p| p != 0)) else {
        return Ok(out);
    };
    let list = res.u64(lod + 0xD0)?;
    let mut seen = Vec::new();
    for i in 0..res.u8(lod + 0x11E)? as u64 {
        let child = res.u64(list + i * 8)?;
        if child == 0 {
            continue;
        }
        let ptr = res.u64(child + 0xA0)?;
        if ptr == 0 || seen.contains(&ptr) || res.u64(ptr + 0x50)? == 0 {
            continue;
        }
        seen.push(ptr);
        let mut d = read_drawable_at(res, ptr, enhanced)?;
        d.name = format!("{name}_child{}", out.children.len());
        out.children.push((res.u16(child + 0x12)?, d));
    }
    Ok(out)
}

/// A `.ydd` file: its drawables, named after their hashes where the name is known.
pub fn read_ydd(res: &Resource) -> Result<Vec<OfDrawable>, ResourceError> {
    let enhanced = enhanced(res)?;
    let hashes = res.u64(SYS + 0x20)?;
    let list = res.u64(SYS + 0x30)?;
    let mut out = Vec::new();
    for i in 0..(res.u16(SYS + 0x38)? as u64).min(4096) {
        let hash = if hashes != 0 { res.u32(hashes + i * 4)? } else { 0 };
        let mut d = read_drawable_at(res, res.u64(list + i * 8)?, enhanced)?;
        d.name = crate::names::value(hash);
        out.push(d);
    }
    Ok(out)
}

/// A `.ytd` file.
pub fn read_ytd(res: &Resource) -> Result<Vec<OfTexture>, ResourceError> {
    let dict = ytd::parse(res)?;
    if let Some(e) = dict.errors.first() {
        return Err(bad(format!("a texture cannot be read: {e}")));
    }
    Ok(dict.textures.into_iter().map(|texture| OfTexture { texture }).collect())
}

fn vec3(res: &Resource, at: u64) -> Result<[f32; 3], ResourceError> {
    Ok([f(res, at)?, f(res, at + 4)?, f(res, at + 8)?])
}

fn f(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    Ok(f32::from_bits(res.u32(at)?))
}

/// The drawable structure at `ptr` (a `.ydr`'s root, a fragment's drawable, …).
pub fn read_drawable_at(res: &Resource, ptr: u64, enhanced: bool) -> Result<OfDrawable, ResourceError> {
    let mut d = OfDrawable {
        center: vec3(res, ptr + 0x20)?,
        radius: f(res, ptr + 0x2C)?,
        aabb_min: vec3(res, ptr + 0x30)?,
        aabb_max: vec3(res, ptr + 0x40)?,
        ..Default::default()
    };
    let group = res.u64(ptr + 0x10)?;
    if group != 0 {
        d.shaders = read_shaders(res, group, enhanced)?;
        let txd = res.u64(group + 0x08)?;
        // A few of the game's models have broken embedded dictionaries (the
        // game does not use them either): what can be read is exported.
        if txd != 0
            && let Ok(dict) = ytd::parse_at(res, txd, enhanced)
        {
            d.textures = dict.textures.into_iter().map(|texture| OfTexture { texture }).collect();
        }
    }
    let skel = res.u64(ptr + 0x18)?;
    if skel != 0 {
        d.skeleton = Some(read_skeleton(res, skel)?);
    }
    for (k, lod) in d.lods.iter_mut().enumerate() {
        let k = k as u64;
        lod.distance = f(res, ptr + 0x70 + k * 4)?;
        let mask = (res.u32(ptr + 0x80 + k * 4)? >> 8 & 0xFF) as u8;
        let list = res.u64(ptr + 0x50 + k * 8)?;
        if list == 0 {
            continue;
        }
        let models = res.u64(list)?;
        for m in 0..res.u16(list + 8)? as u64 {
            let model = res.u64(models + m * 8)?;
            if model != 0 {
                let mut model = read_model(res, model, enhanced)?;
                model.mask = mask;
                lod.models.push(model);
            }
        }
    }
    Ok(d)
}

fn read_skeleton(res: &Resource, ptr: u64) -> Result<OfSkeleton, ResourceError> {
    let bones = res.u64(ptr + 0x20)?;
    let count = res.u16(ptr + 0x5E)? as u64;
    if bones == 0 || count > 2048 {
        return Err(bad("invalid skeleton"));
    }
    let mut out = OfSkeleton { crc: res.u32(ptr + 0x58)?, bones: Vec::new() };
    for i in 0..count {
        let b = bones + i * 0x50;
        let name = res.u64(b + 0x38)?;
        let tag = res.u16(b + 0x44)?;
        out.bones.push(OfBone {
            name: if name != 0 { res.string(name)? } else { String::new() },
            tag,
            mirror: tag,
            parent: res.u16(b + 0x32)? as i16,
            flags: res.u16(b + 0x40)?,
            rotation: [f(res, b)?, f(res, b + 4)?, f(res, b + 8)?, f(res, b + 12)?],
            translation: vec3(res, b + 0x10)?,
            scale: vec3(res, b + 0x20)?,
        });
    }
    Ok(out)
}

fn read_model(res: &Resource, model: u64, enhanced: bool) -> Result<OfModel, ResourceError> {
    let binding = res.u32(model + 0x28)?;
    let skinned = binding >> 8 & 0xFF != 0;
    let mut out = OfModel {
        bone: if skinned { 0 } else { (binding >> 24) as u8 },
        skinned,
        bone_count: (binding & 0xFF) as u8,
        ..Default::default()
    };
    let geoms = res.u64(model + 0x08)?;
    let count = res.u16(model + 0x10)? as u64;
    let mapping = res.u64(model + 0x20)?;
    let boxes = res.u64(model + 0x18)?;
    if boxes != 0 {
        for i in 0..if count > 1 { count + 1 } else { count } {
            out.bounds.push((vec3(res, boxes + i * 32)?, vec3(res, boxes + i * 32 + 16)?));
        }
    }
    for i in 0..count {
        let shader = if mapping != 0 { res.u16(mapping + i * 2)? } else { 0 };
        let g = read_geometry(res, res.u64(geoms + i * 8)?, enhanced)?;
        out.geometries.push(OfGeometry { shader, ..g });
    }
    Ok(out)
}

/// Where each component of a vertex is and how it is stored.
struct Layout {
    decl: VertexDecl,
    /// (component, byte offset, encoding)
    slots: Vec<(usize, usize, Encoding)>,
}

fn legacy_type(t: u8) -> Option<(usize, Option<Encoding>)> {
    Some(match t {
        1 => (4, Some(Encoding::Half2)),
        2 => (4, None), // float
        3 => (8, Some(Encoding::Half4)),
        5 => (8, Some(Encoding::Float2)),
        6 => (12, Some(Encoding::Float3)),
        7 => (16, Some(Encoding::Float4)),
        8 => (4, Some(Encoding::Uint8x4)),
        9 => (4, Some(Encoding::Unorm8x4)),
        10 => (4, Some(Encoding::Snorm8x4)),
        _ => return None,
    })
}

fn legacy_layout(res: &Resource, decl: u64) -> Result<Layout, ResourceError> {
    let decl = VertexDecl { flags: res.u32(decl)? & 0xFFFF, types: res.u64(decl + 8)? };
    let mut slots = Vec::new();
    let mut offset = 0;
    for (k, t) in decl.components() {
        let (size, enc) = legacy_type(t).ok_or_else(|| bad(format!("unknown vertex component type {t}")))?;
        let enc = enc.ok_or_else(|| bad(format!("vertex component {k} has type {t}, which is not read")))?;
        slots.push((k, offset, enc));
        offset += size;
    }
    Ok(Layout { decl, slots })
}

/// Enhanced declarations: 52 slots with offsets and DXGI formats. Each slot
/// that holds something becomes the matching Legacy component with its
/// usual type.
fn enhanced_layout(res: &Resource, decl: u64) -> Result<Layout, ResourceError> {
    let offsets = res.bytes(decl, 52 * 4)?;
    let formats = res.bytes(decl + 52 * 4 + 52, 52)?;
    let component = |slot: usize| match slot {
        0 => Some(component::POSITION),
        4 => Some(component::NORMAL),
        8 => Some(component::TANGENT),
        12 => Some(component::BINORMAL),
        16 => Some(component::BLEND_WEIGHTS),
        20 => Some(component::BLEND_INDICES),
        24 => Some(component::COLOUR0),
        25 => Some(component::COLOUR1),
        28..=35 => Some(component::TEXCOORD0 + slot - 28),
        _ => None,
    };
    let encoding = |format: u8| match format {
        2 => Some(Encoding::Float4),
        6 => Some(Encoding::Float3),
        10 => Some(Encoding::Half4),
        16 => Some(Encoding::Float2),
        24 => Some(Encoding::R10G10B10A2),
        28 => Some(Encoding::Unorm8x4),
        30 => Some(Encoding::Uint8x4),
        34 => Some(Encoding::Half2),
        _ => None,
    };
    let mut slots = Vec::new();
    let mut flags = 0u32;
    for slot in 0..52 {
        let format = formats[slot];
        if format == 0 {
            continue;
        }
        let Some(k) = component(slot) else { continue };
        let enc = encoding(format).ok_or_else(|| bad(format!("vertex slot {slot} has format {format}")))?;
        let off = u32::from_le_bytes(offsets[slot * 4..slot * 4 + 4].try_into().expect("4 bytes")) as usize;
        flags |= 1 << k;
        slots.push((k, off, enc));
    }
    slots.sort_by_key(|s| s.0);
    Ok(Layout { decl: VertexDecl { flags, types: DEFAULT_TYPES }, slots })
}

fn read_geometry(res: &Resource, g: u64, enhanced: bool) -> Result<OfGeometry, ResourceError> {
    let (vb, ib) = (res.u64(g + 0x18)?, res.u64(g + 0x38)?);
    let (count, stride, data, layout) = if enhanced {
        (
            res.u32(vb + 0x08)? as usize,
            res.u16(vb + 0x0C)? as usize,
            res.u64(vb + 0x18)?,
            enhanced_layout(res, res.u64(vb + 0x38)?)?,
        )
    } else {
        (
            res.u32(vb + 0x18)? as usize,
            res.u16(vb + 0x08)? as usize,
            res.u64(vb + 0x10)?,
            legacy_layout(res, res.u64(vb + 0x30)?)?,
        )
    };
    if stride == 0 || count > 1 << 22 {
        return Err(bad("implausible vertex buffer"));
    }
    if layout.slots.iter().any(|&(_, o, e)| o + e.size() > stride) {
        return Err(bad("a vertex component lies outside the vertex"));
    }
    let bytes = res.bytes(data, count * stride)?;
    // Blend indices point into the geometry's bone table.
    let table: Vec<u16> = {
        let table = res.u64(g + 0x68)?;
        let n = res.u16(g + 0x72)? as u64;
        if table != 0 && n <= 1024 {
            (0..n).map(|i| res.u16(table + i * 2)).collect::<Result<_, _>>()?
        } else {
            Vec::new()
        }
    };
    let mut attributes = Vec::new();
    for &(k, off, enc) in &layout.slots {
        let mut values = Vec::with_capacity(count);
        for i in 0..count {
            let raw = &bytes[i * stride + off..i * stride + off + enc.size()];
            let v = match (k, enc) {
                // Bytes as they are stored.
                (component::BLEND_INDICES | component::COLOUR0 | component::COLOUR1, _) if enc.size() == 4 => {
                    [raw[0], raw[1], raw[2], raw[3]].map(f32::from)
                }
                _ => enc.decode(raw),
            };
            values.push(v);
        }
        if k == component::BLEND_INDICES && !table.is_empty() {
            for v in &mut values {
                *v = v.map(|i| table.get(i as usize).copied().map_or(i, f32::from));
            }
        }
        attributes.push((k, values));
    }
    let index_count = res.u32(ib + 0x08)? as usize;
    let (ptr, size) =
        if enhanced { (res.u64(ib + 0x18)?, res.u16(ib + 0x0C)?.max(2) as usize) } else { (res.u64(ib + 0x10)?, 2) };
    let raw = res.bytes(ptr, index_count * size)?;
    let indices: Vec<u32> = if size == 4 {
        raw.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
    } else {
        raw.as_chunks::<2>().0.iter().map(|c| u32::from(u16::from_le_bytes(*c))).collect()
    };
    if indices.iter().any(|&i| i as usize >= count) {
        return Err(bad("index outside the vertex buffer"));
    }
    Ok(OfGeometry { shader: 0, decl: layout.decl, indices, attributes })
}

fn read_shaders(res: &Resource, group: u64, enhanced: bool) -> Result<Vec<OfShader>, ResourceError> {
    let list = res.u64(group + 0x10)?;
    let count = res.u16(group + 0x18)? as u64;
    let texture_name = |ptr: u64| -> Result<String, ResourceError> {
        let name = res.u64(ptr + 0x28)?;
        Ok(if name != 0 { res.string(name)? } else { String::new() })
    };
    let mut out = Vec::new();
    for i in 0..count.min(512) {
        let s = res.u64(list + i * 8)?;
        let mut shader = OfShader::default();
        if enhanced {
            shader.name = shader_name(res.u32(s)?);
            let ptrs = res.u64(s + 0x10)?;
            let infos = res.u64(s + 0x20)?;
            if infos != 0 && ptrs != 0 {
                let total = res.u8(infos + 4)? as u64;
                for k in 0..total.min(128) {
                    let (hash, slot) = (res.u32(infos + 8 + k * 8)?, res.u32(infos + 12 + k * 8)?);
                    if slot & 3 != 0 {
                        continue;
                    }
                    let p = res.u64(ptrs + u64::from(slot >> 2) * 8)?;
                    if p != 0 {
                        shader
                            .params
                            .push((legacy_texture_param(&param_name(hash).0), OfValue::Texture(texture_name(p)?)));
                    }
                }
            }
        } else {
            // Parameters: 16 bytes each (data type: 0 texture, else the
            // number of float4s; data pointer at 8), then their data, then
            // one name hash per parameter.
            let params = res.u64(s)?;
            shader.name = shader_name(res.u32(s + 0x08)?);
            let n = res.u8(s + 0x10)? as u64;
            let types: Vec<u64> = (0..n).map(|p| res.u8(params + p * 16).map(u64::from)).collect::<Result<_, _>>()?;
            let hashes = params + n * 16 + types.iter().sum::<u64>() * 16;
            for (p, &t) in types.iter().enumerate() {
                let p = p as u64;
                let (name, width) = param_name(res.u32(hashes + p * 4)?);
                let data = res.u64(params + p * 16 + 8)?;
                let value = if t == 0 {
                    if data == 0 {
                        continue;
                    }
                    OfValue::Texture(texture_name(data)?)
                } else {
                    let mut v = (0..t * 4).map(|k| f(res, data + k * 4)).collect::<Result<Vec<_>, _>>()?;
                    if t == 1
                        && let Some(w) = width
                    {
                        v.truncate(w.max(1));
                    }
                    OfValue::Floats(v)
                };
                shader.params.push((name, value));
            }
        }
        out.push(shader);
    }
    Ok(out)
}

/// GTA V Enhanced names its texture parameters `…Tex`, Legacy (and OpenIV's
/// files) `…Sampler`: the Legacy name where it is known.
pub(crate) fn legacy_texture_param(name: &str) -> String {
    let legacy = match name {
        "SpecularTex" => "SpecSampler".to_owned(),
        "DamageTexture" => "DamageSampler".to_owned(),
        n => {
            if let Some(base) = n.strip_suffix("Tex2") {
                format!("{base}Sampler2")
            } else if let Some(base) = n.strip_suffix("Tex") {
                format!("{base}Sampler")
            } else {
                return n.to_owned();
            }
        }
    };
    if PARAMS.iter().any(|(p, _)| *p == legacy) { legacy } else { name.to_owned() }
}
