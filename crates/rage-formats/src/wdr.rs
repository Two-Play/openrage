//! GTA IV models: drawables (`.wdr`), drawable dictionaries (`.wdd`) and
//! fragments (`.wft`), RSC5 resources with 32-bit pointers.
//!
//! The structures are the 32-bit ancestors of GTA V Legacy's:
//! * drawable: `0x08` shader group, `0x0C` skeleton, `0x20`/`0x30` bounds,
//!   `0x40..0x50` model lists of the four LODs (pointer to {pointer, u16 count});
//! * shader group: `0x04` embedded texture dictionary, `0x08` shaders;
//! * shader: `0x14` parameter data pointers, `0x1C` count, `0x24` parameter
//!   types (0 = texture), `0x28` name hash, `0x34` parameter name hashes,
//!   `0x44` shader name; a texture reference keeps its name at `0x14`;
//! * model: `0x04` geometries, `0x10` shader index per geometry, `0x15`
//!   skinned flag;
//! * geometry: `0x0C` vertex buffer, `0x1C` index buffer, `0x38` bone table
//!   (u16 skeleton indices) with its count at `0x3E`;
//! * vertex buffer: `0x04` count, `0x08` data, `0x0C` stride, `0x10`
//!   declaration (the same as GTA V Legacy's); index buffer: `0x04` count,
//!   `0x08` data (u16);
//! * skeleton: `0x00` bones (0xE0 bytes each: name at `0x00`, parent pointer at
//!   `0x10`, tag at `0x16`, translation at `0x20`, rotation quaternion at
//!   `0x40`), count at `0x14`;
//! * drawable dictionary: `0x10` name hashes, `0x18` drawables;
//! * fragment: `0xB0` name, `0xB4` main drawable, `0xD4` children (each with
//!   its skeleton bone index at `0x0E` and its drawable at `0x90`), collision
//!   bound at `0xEC`, child count at `0x1F2`.

use rage_resource::{Resource, ResourceError};

use crate::drawable::{
    Bone, Drawable, Encoding, Fragment, FragmentChild, Mesh, Shader, Skeleton, bad, legacy_layout, vec3,
};
use crate::ytd;

/// RSC5 resource types of GTA IV models.
pub const TYPE_DRAWABLE: u32 = 110;
pub const TYPE_FRAGMENT: u32 = 112;

const MAX_GEOMETRIES: u64 = 4096;

fn ptr(res: &Resource, at: u64) -> Result<u64, ResourceError> {
    res.u32(at).map(u64::from)
}

/// A non-null pointer that lies inside the resource.
fn valid(res: &Resource, p: u64) -> bool {
    p != 0 && res.bytes(p, 1).is_ok()
}

/// A pointer list header (`{pointer, u16 count}`) as addresses.
fn pointer_list(res: &Resource, header: u64) -> Result<Vec<u64>, ResourceError> {
    let list = ptr(res, header)?;
    let count = res.u16(header + 4)? as u64;
    if count > MAX_GEOMETRIES {
        return Err(bad(format!("implausible list of {count} entries")));
    }
    if list == 0 {
        return Ok(Vec::new());
    }
    (0..count).map(|i| ptr(res, list + i * 4)).collect()
}

fn read_geometry(res: &Resource, g: u64) -> Result<Mesh, ResourceError> {
    let vb = ptr(res, g + 0x0C)?;
    let ib = ptr(res, g + 0x1C)?;
    let count = res.u16(vb + 0x04)? as usize;
    let data = ptr(res, vb + 0x08)?;
    let stride = res.u32(vb + 0x0C)? as usize;
    if stride == 0 || stride > 256 {
        return Err(bad("implausible vertex buffer"));
    }
    let mut layout = legacy_layout(res, ptr(res, vb + 0x10)?, stride)?;
    // Blend indices are stored as plain bytes.
    if let Some((o, Encoding::Unorm8x4)) = layout.indices {
        layout.indices = Some((o, Encoding::Uint8x4));
    }
    let bytes = res.bytes(data, count * stride)?;
    let (pos_off, pos_enc) = layout.position.ok_or_else(|| bad("vertices without positions"))?;
    let attr = |slot: Option<(usize, Encoding)>, i: usize| {
        slot.filter(|(o, e)| o + e.size() <= stride).map(|(o, e)| e.decode(&bytes[i * stride + o..]))
    };
    if pos_off + pos_enc.size() > stride {
        return Err(bad("position outside vertex"));
    }
    let mut mesh = Mesh::default();
    for i in 0..count {
        let p = pos_enc.decode(&bytes[i * stride + pos_off..]);
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
    if !mesh.bone_indices.is_empty() {
        let table = ptr(res, g + 0x38)?;
        let n = res.u16(g + 0x3E)? as u64;
        if valid(res, table) && n > 0 && n <= 1024 {
            let ids: Vec<u16> = (0..n).map(|i| res.u16(table + i * 2)).collect::<Result<_, _>>()?;
            for b in &mut mesh.bone_indices {
                *b = b.map(|i| ids.get(i as usize).copied().unwrap_or(0));
            }
        }
    }
    let index_count = res.u32(ib + 0x04)? as usize;
    let ib_bytes = res.bytes(ptr(res, ib + 0x08)?, index_count * 2)?;
    mesh.indices = ib_bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c) as u32).collect();
    if mesh.indices.iter().any(|&i| i as usize >= count) {
        return Err(bad("index outside the vertex buffer"));
    }
    mesh.indices.truncate(mesh.indices.len() / 3 * 3);
    Ok(mesh)
}

fn read_models(res: &Resource, header: u64, out: &mut Vec<Mesh>) -> Result<(), ResourceError> {
    for model in pointer_list(res, header)? {
        if model == 0 {
            continue;
        }
        let geometries = pointer_list(res, model + 0x04)?;
        let mapping = ptr(res, model + 0x10)?;
        let skinned = res.u8(model + 0x15)? != 0;
        for (i, g) in geometries.into_iter().enumerate() {
            let mut mesh = read_geometry(res, g)?;
            mesh.shader = if mapping != 0 { res.u16(mapping + i as u64 * 2)? } else { 0 };
            if !skinned {
                mesh.bone_indices.clear();
                mesh.bone_weights.clear();
            }
            out.push(mesh);
        }
    }
    Ok(())
}

/// A string at `p`, if it looks like a name.
fn name_at(res: &Resource, p: u64) -> Option<String> {
    let name = res.string(p).ok()?;
    (!name.is_empty() && name.len() < 128 && name.chars().all(|c| c.is_ascii_graphic())).then_some(name)
}

fn read_shaders(res: &Resource, group: u64) -> Result<Vec<Shader>, ResourceError> {
    let mut shaders = Vec::new();
    for s in pointer_list(res, group + 0x08)? {
        let mut shader = Shader { name_hash: res.u32(s + 0x28)?, ..Default::default() };
        let params = ptr(res, s + 0x14)?;
        let count = res.u32(s + 0x1C)?.min(128) as u64;
        let types = ptr(res, s + 0x24)?;
        let hashes = ptr(res, s + 0x34)?;
        for p in 0..count {
            if res.u8(types + p)? != 0 {
                continue;
            }
            let data = ptr(res, params + p * 4)?;
            if !valid(res, data) {
                continue;
            }
            let Some(name) = ptr(res, data + 0x14).ok().filter(|&n| valid(res, n)).and_then(|n| name_at(res, n)) else {
                continue;
            };
            let name = ytd::gta4_texture_name(&name).to_owned();
            shader.samplers.push((res.u32(hashes + p * 4).unwrap_or(0), name.clone()));
            if !shader.textures.contains(&name) {
                shader.textures.push(name);
            }
        }
        shaders.push(shader);
    }
    Ok(shaders)
}

/// Name of a shader (`gta_default`, `gta_spec`, …).
pub fn shader_name(res: &Resource, shader: u64) -> Option<String> {
    name_at(res, ptr(res, shader + 0x44).ok()?)
}

const BONE_SIZE: u64 = 0xE0;

fn read_skeleton(res: &Resource, s: u64) -> Result<Skeleton, ResourceError> {
    let bones = ptr(res, s)?;
    let count = res.u16(s + 0x14)? as u64;
    if !valid(res, bones) || count > 2048 {
        return Err(bad("invalid skeleton"));
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let b = bones + i * BONE_SIZE;
        let f = |o: u64| -> Result<f32, ResourceError> { Ok(f32::from_bits(res.u32(b + o)?)) };
        let parent = ptr(res, b + 0x10)?;
        let parent = if parent >= bones && (parent - bones) % BONE_SIZE == 0 && (parent - bones) / BONE_SIZE < i {
            ((parent - bones) / BONE_SIZE) as i16
        } else {
            -1
        };
        let name_ptr = ptr(res, b)?;
        out.push(Bone {
            name: if valid(res, name_ptr) { res.string(name_ptr).unwrap_or_default() } else { String::new() },
            tag: res.u16(b + 0x16)?,
            parent,
            translation: [f(0x20)?, f(0x24)?, f(0x28)?],
            rotation: [f(0x40)?, f(0x44)?, f(0x48)?, f(0x4C)?],
            scale: [1.0; 3],
        });
    }
    Ok(Skeleton { bones: out })
}

/// Reads the drawable structure at `p`.
pub fn read_drawable(res: &Resource, p: u64) -> Result<Drawable, ResourceError> {
    let mut d = Drawable { bounds_min: vec3(res, p + 0x20)?, bounds_max: vec3(res, p + 0x30)?, ..Default::default() };
    let skel = ptr(res, p + 0x0C)?;
    if valid(res, skel) {
        d.skeleton = read_skeleton(res, skel).ok();
    }
    for lod in 0..4 {
        let header = ptr(res, p + 0x40 + lod * 4)?;
        if valid(res, header) {
            read_models(res, header, &mut d.meshes)?;
            if !d.meshes.is_empty() {
                break;
            }
        }
    }
    let group = ptr(res, p + 0x08)?;
    if valid(res, group) {
        d.shaders = read_shaders(res, group).unwrap_or_default();
        let txd = ptr(res, group + 0x04)?;
        if valid(res, txd) {
            d.textures = ytd::parse_gta4_at(res, txd).ok();
        }
    }
    Ok(d)
}

/// A `.wdr` file.
pub fn parse_wdr(res: &Resource) -> Result<Drawable, ResourceError> {
    read_drawable(res, Resource::ROOT)
}

/// A `.wdd` file: (name hash, drawable) pairs.
pub fn parse_wdd(res: &Resource) -> Result<Vec<(u32, Drawable)>, ResourceError> {
    let root = Resource::ROOT;
    let hashes = ptr(res, root + 0x10)?;
    let drawables = pointer_list(res, root + 0x18)?;
    let mut out = Vec::with_capacity(drawables.len());
    for (i, p) in drawables.into_iter().enumerate() {
        let hash = if hashes != 0 { res.u32(hashes + i as u64 * 4)? } else { 0 };
        out.push((hash, read_drawable(res, p)?));
    }
    Ok(out)
}

/// The name a fragment keeps for itself (`pack:/admiral` → `admiral`).
pub fn fragment_name(res: &Resource) -> Option<String> {
    let n = name_at(res, ptr(res, Resource::ROOT + 0xB0).ok()?)?;
    Some(n.strip_prefix("pack:/").unwrap_or(&n).to_owned())
}

/// The collision bound of a fragment (`0xEC`, usually a composite with one
/// part per breakable or moving piece), read with [`crate::wbn::parse_bound`].
pub fn fragment_bound(res: &Resource) -> Option<u64> {
    let at = ptr(res, Resource::ROOT + 0xEC).ok()?;
    (valid(res, at) && matches!(res.u8(at + 0x04).ok()?, 0 | 1 | 3 | 4 | 10 | 12)).then_some(at)
}

/// A `.wft` file: the main drawable and the children that have meshes of
/// their own (vehicle wheels, …), attached to bones of the main skeleton.
pub fn parse_wft(res: &Resource) -> Result<Fragment, ResourceError> {
    let root = Resource::ROOT;
    let main = ptr(res, root + 0xB4)?;
    if !valid(res, main) {
        return Err(bad("fragment without a drawable"));
    }
    let mut drawable = read_drawable(res, main)?;
    drawable.name = fragment_name(res).unwrap_or_default();
    let mut fragment = Fragment { drawable, children: Vec::new() };
    let list = ptr(res, root + 0xD4)?;
    let count = res.u8(root + 0x1F2)? as u64;
    if !valid(res, list) {
        return Ok(fragment);
    }
    for i in 0..count {
        let c = ptr(res, list + i * 4)?;
        if !valid(res, c) {
            continue;
        }
        let d = ptr(res, c + 0x90)?;
        if !valid(res, d) {
            continue;
        }
        let mut child = read_drawable(res, d)?;
        if child.meshes.is_empty() {
            continue;
        }
        // Children (wheels) use the main drawable's shaders.
        if child.shaders.is_empty() {
            child.shaders = fragment.drawable.shaders.clone();
            child.textures = fragment.drawable.textures.clone();
        }
        let bone = res.u16(c + 0x0E)? as usize;
        let tag = fragment.drawable.skeleton.as_ref().and_then(|s| s.bones.get(bone)).map_or(0, |b| b.tag);
        fragment.children.push(FragmentChild { bone_tag: tag, drawable: child });
    }
    Ok(fragment)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);

    impl Mem {
        fn put(&mut self, at: usize, v: u32) {
            self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        fn put16(&mut self, at: usize, v: u16) {
            self.0[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
        fn putf(&mut self, at: usize, v: f32) {
            self.put(at, v.to_bits());
        }
    }

    const S: u32 = 0x5000_0000;
    const G: u32 = 0x6000_0000;

    /// A drawable with one triangle (position + uv) and one textured shader.
    fn drawable() -> Resource {
        let mut m = Mem(vec![0; 0x400]);
        m.put(0x08, S + 0x100); // shader group
        m.put(0x40, S + 0x200); // high LOD list header
        // shader group: shaders {ptr, count}
        m.put(0x100 + 0x08, S + 0x120);
        m.put16(0x100 + 0x0C, 1);
        m.put(0x120, S + 0x140);
        // shader
        let sh = 0x140;
        m.put(sh + 0x14, S + 0x1A0); // param data pointers
        m.put(sh + 0x1C, 1);
        m.put(sh + 0x24, S + 0x1A8); // types (0 = texture)
        m.put(sh + 0x28, 0x1234);
        m.put(sh + 0x34, S + 0x1B0); // name hashes
        m.put(0x1A0, S + 0x1C0); // texture reference
        m.put(0x1B0, crate::drawable::param::TEXTURE_SAMPLER);
        m.put(0x1C0 + 0x14, S + 0x1E0);
        m.0[0x1E0..0x1E0 + 6].copy_from_slice(b"brick\0");
        // model list header → model
        m.put(0x200, S + 0x210);
        m.put16(0x204, 1);
        m.put(0x210, S + 0x220);
        let model = 0x220;
        m.put(model + 0x04, S + 0x250); // geometries
        m.put16(model + 0x08, 1);
        m.put(model + 0x10, S + 0x258); // shader mapping (0)
        m.put(0x250, S + 0x260);
        let g = 0x260;
        m.put(g + 0x0C, S + 0x2B0); // vertex buffer
        m.put(g + 0x1C, S + 0x2D0); // index buffer
        let vb = 0x2B0;
        m.put16(vb + 0x04, 3);
        m.put(vb + 0x08, G);
        m.put(vb + 0x0C, 20);
        m.put(vb + 0x10, S + 0x2F0);
        // declaration: position (float3) and uv0 (float2)
        m.put(0x2F0, 1 | 1 << 6);
        m.put(0x2F0 + 8, 6 | 5 << 24);
        let ib = 0x2D0;
        m.put(ib + 0x04, 3);
        m.put(ib + 0x08, G + 60);
        let mut gfx = Mem(vec![0; 66]);
        for (i, p) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].iter().enumerate() {
            for (k, v) in p.iter().enumerate() {
                gfx.putf(i * 20 + k * 4, *v);
            }
            gfx.putf(i * 20 + 12, i as f32);
        }
        for i in 0..3 {
            gfx.put16(60 + i * 2, i as u16);
        }
        Resource { version: TYPE_DRAWABLE, system: m.0, graphics: gfx.0 }
    }

    #[test]
    fn reads_a_drawable() {
        let d = parse_wdr(&drawable()).unwrap();
        assert_eq!(d.meshes.len(), 1);
        let mesh = &d.meshes[0];
        assert_eq!(mesh.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(mesh.uvs[2], [2.0, 0.0]);
        assert_eq!(mesh.indices, [0, 1, 2]);
        assert_eq!(d.shaders.len(), 1);
        assert_eq!(d.shaders[0].base_texture(), Some("brick"));
        assert_eq!(d.triangle_count(), 1);
    }
}
