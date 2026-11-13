//! GTA IV collision bounds: `.wbn` (one bound) and `.wbd` (a dictionary of
//! bounds), RSC5 resources with 32-bit pointers, turned into meshes like
//! [`crate::ybn`].
//!
//! * `.wbn` root: `0x08` the bound. `.wbd` root: `0x10` name hashes, `0x18`
//!   bounds (pointer, u16 count).
//! * Every bound: type at `0x04`, radius `0x08`, box max `0x10`, box min
//!   `0x20`, centre `0x30`. Types: 0 sphere, 1 capsule, 3 box, 4 geometry,
//!   5 curved geometry (wheels, drawn as a cylinder filling the box along its
//!   shortest side; material from the list at `0xD0`), 10 geometry with BVH,
//!   12 composite.
//! * Sphere: centre `0x30`, radius `0x08`. Capsule (along Y): centre `0x30`,
//!   radius `0x80`, length of the straight part `0x90`.
//! * Box, geometry, BVH (polyhedra): polygons `0x8C` (32 bytes: normal,
//!   area — whose lowest byte is the polygon's material index — then four u16
//!   vertex indices and four neighbours; a fourth index of 0 off the
//!   polygon's plane makes it a triangle), quantum `0x90`, offset `0xA0`,
//!   vertices `0xB0` (3 × i16, × quantum + offset), vertex count `0xC8`,
//!   polygon count `0xCC`; geometry and BVH: materials `0xD0` (u32 each,
//!   type in the low byte), material count `0xD8`.
//! * Composite: children `0x80`, matrices `0x84` (4 × 4 floats: axes then
//!   translation, w components unused), child count `0x90` (u16).

use rage_resource::{Resource, ResourceError};

use crate::drawable::Drawable;
use crate::geometry::{Builder, add, dot, sub};
use crate::math::{self, Mat4};
use crate::ybn::{Stats, finish};

/// Bound types of GTA IV.
pub mod kind {
    pub const SPHERE: u8 = 0;
    pub const CAPSULE: u8 = 1;
    pub const BOX: u8 = 3;
    pub const GEOMETRY: u8 = 4;
    /// Curved geometry: vehicle wheels.
    pub const CURVED: u8 = 5;
    pub const BVH: u8 = 10;
    pub const COMPOSITE: u8 = 12;
}

/// Shader name hashes of GTA IV collision materials: this base + the material
/// type (an index into GTA IV's `materials.dat`, which differs from GTA V's).
pub const MATERIAL_BASE: u32 = 0xC014_0000;

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

fn ptr(res: &Resource, at: u64) -> Result<u64, ResourceError> {
    res.u32(at).map(u64::from)
}

fn f32_at(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    Ok(f32::from_bits(res.u32(at)?))
}

fn vec3(res: &Resource, at: u64) -> Result<[f32; 3], ResourceError> {
    Ok([f32_at(res, at)?, f32_at(res, at + 4)?, f32_at(res, at + 8)?])
}

fn bound(
    res: &Resource,
    at: u64,
    m: &Mat4,
    b: &mut Builder,
    stats: &mut Stats,
    depth: usize,
) -> Result<(), ResourceError> {
    if depth > 8 {
        return Err(bad("bounds nested too deeply"));
    }
    stats.bounds += 1;
    match res.u8(at + 0x04)? {
        kind::COMPOSITE => {
            let children = ptr(res, at + 0x80)?;
            let matrices = ptr(res, at + 0x84)?;
            let count = res.u16(at + 0x90)? as u64;
            for i in 0..count.min(4096) {
                let child = ptr(res, children + i * 4)?;
                if child == 0 {
                    continue;
                }
                let mut t = math::IDENTITY;
                if matrices != 0 {
                    let raw = res.bytes(matrices + i * 64, 64)?;
                    for (k, c) in raw.as_chunks::<4>().0.iter().enumerate() {
                        t[k] = f32::from_le_bytes(*c);
                    }
                    (t[3], t[7], t[11], t[15]) = (0.0, 0.0, 0.0, 1.0);
                }
                bound(res, child, &math::mul(m, &t), b, stats, depth + 1)?;
            }
        }
        kind::BOX | kind::GEOMETRY | kind::BVH => polyhedron(res, at, m, b, stats)?,
        kind::CURVED => {
            let (hi, lo) = (vec3(res, at + 0x10)?, vec3(res, at + 0x20)?);
            let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
            let half = [(hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0, (hi[2] - lo[2]) / 2.0];
            let axis = (0..3).min_by(|&i, &j| half[i].total_cmp(&half[j])).unwrap_or(0);
            let radius = (0..3).filter(|&i| i != axis).map(|i| half[i]).fold(0.0, f32::max);
            let mut d = [0.0; 3];
            d[axis] = half[axis];
            let list = ptr(res, at + 0xD0)?;
            let material = if list != 0 { res.u8(list)? } else { 0 };
            b.round(material, m, sub(c, d), add(c, d), radius, false);
        }
        kind::SPHERE => {
            let c = vec3(res, at + 0x30)?;
            b.round(0, m, c, c, f32_at(res, at + 0x08)?, true);
        }
        kind::CAPSULE => {
            let c = vec3(res, at + 0x30)?;
            let half = f32_at(res, at + 0x90)? / 2.0;
            b.round(0, m, sub(c, [0.0, half, 0.0]), add(c, [0.0, half, 0.0]), f32_at(res, at + 0x80)?, true);
        }
        t => return Err(bad(format!("unknown bound type {t}"))),
    }
    Ok(())
}

fn polyhedron(res: &Resource, at: u64, m: &Mat4, b: &mut Builder, stats: &mut Stats) -> Result<(), ResourceError> {
    let kind = res.u8(at + 0x04)?;
    let polys = ptr(res, at + 0x8C)?;
    let quantum = vec3(res, at + 0x90)?;
    let offset = vec3(res, at + 0xA0)?;
    let verts_at = ptr(res, at + 0xB0)?;
    let vert_count = res.u32(at + 0xC8)? as u64;
    let poly_count = res.u32(at + 0xCC)? as u64;
    if vert_count > 1 << 20 || poly_count > 1 << 21 {
        return Err(bad(format!("implausible geometry: {vert_count} vertices, {poly_count} polygons")));
    }
    // Boxes keep no material list.
    let types: Vec<u8> = if kind == kind::BOX {
        vec![0]
    } else {
        let at_list = ptr(res, at + 0xD0)?;
        let count = res.u32(at + 0xD8)?.clamp(1, 256) as u64;
        (0..count).map(|i| if at_list != 0 { res.u8(at_list + i * 4).unwrap_or(0) } else { 0 }).collect()
    };
    stats.vertices += vert_count as usize;
    let raw = if vert_count > 0 { res.bytes(verts_at, vert_count as usize * 6)? } else { &[][..] };
    let verts: Vec<[f32; 3]> = raw
        .as_chunks::<6>()
        .0
        .iter()
        .map(|c| {
            let q = [0, 2, 4].map(|o| i16::from_le_bytes([c[o], c[o + 1]]) as f32);
            [q[0] * quantum[0] + offset[0], q[1] * quantum[1] + offset[1], q[2] * quantum[2] + offset[2]]
        })
        .collect();
    let data = if poly_count > 0 { res.bytes(polys, poly_count as usize * 32)? } else { &[][..] };
    for p in data.as_chunks::<32>().0 {
        let f = |o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
        let normal = [f(0), f(4), f(8)];
        let material = types.get(p[12] as usize).copied().unwrap_or(types[0]);
        let idx = [16, 18, 20, 22].map(|o| u16::from_le_bytes([p[o], p[o + 1]]) as usize);
        let (Some(&a), Some(&c), Some(&d), Some(&e)) =
            (verts.get(idx[0]), verts.get(idx[1]), verts.get(idx[2]), verts.get(idx[3]))
        else {
            continue;
        };
        b.triangle(material, m, [a, c, d]);
        stats.triangles += 1;
        // A fourth index of 0 that is off the polygon's plane marks a triangle.
        let on_plane = dot(normal, sub(e, a)).abs() < 0.05 * dot(sub(d, a), sub(d, a)).sqrt().max(1.0);
        if idx[3] != 0 || on_plane {
            b.triangle(material, m, [a, d, e]);
            stats.triangles += 1;
        }
    }
    Ok(())
}

/// Reads the bound at `at`.
pub fn parse_bound(res: &Resource, at: u64) -> Result<(Drawable, Stats), ResourceError> {
    let mut b = Builder::default();
    let mut stats = Stats::default();
    bound(res, at, &math::IDENTITY, &mut b, &mut stats, 0)?;
    let (mut d, stats) = finish(b, stats);
    for s in &mut d.shaders {
        s.name_hash = s.name_hash - crate::ybn::MATERIAL_BASE + MATERIAL_BASE;
    }
    Ok((d, stats))
}

/// A `.wbn` file.
pub fn parse_wbn(res: &Resource) -> Result<(Drawable, Stats), ResourceError> {
    let at = ptr(res, Resource::ROOT + 0x08)?;
    if at == 0 {
        return Err(bad("collision file without a bound"));
    }
    parse_bound(res, at)
}

/// A `.wbd` file: (name hash, bound) pairs.
pub fn parse_wbd(res: &Resource) -> Result<Vec<(u32, Drawable, Stats)>, ResourceError> {
    let root = Resource::ROOT;
    let hashes = ptr(res, root + 0x10)?;
    let list = ptr(res, root + 0x18)?;
    let count = res.u16(root + 0x1C)? as u64;
    if count > 1 << 14 {
        return Err(bad(format!("implausible dictionary of {count} bounds")));
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let at = ptr(res, list + i * 4)?;
        if at == 0 {
            continue;
        }
        let hash = if hashes != 0 { res.u32(hashes + i * 4)? } else { 0 };
        let (d, s) = parse_bound(res, at)?;
        out.push((hash, d, s));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(d: &mut [u8], at: usize, v: u32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// A `.wbn` holding a geometry bound: a unit quad and a triangle (fourth index 0, off the plane).
    fn wbn() -> Resource {
        let s = 0x5000_0000u32;
        let mut d = vec![0u8; 0x400];
        put(&mut d, 0x08, s + 0x40);
        let b = 0x40;
        d[b + 4] = kind::GEOMETRY;
        put(&mut d, b + 0x8C, s + 0x200);
        for k in 0..3 {
            put(&mut d, b + 0x90 + k * 4, 1.0f32.to_bits());
        }
        put(&mut d, b + 0xB0, s + 0x300);
        put(&mut d, b + 0xC8, 5);
        put(&mut d, b + 0xCC, 2);
        put(&mut d, b + 0xD0, s + 0x380);
        put(&mut d, b + 0xD8, 2);
        d[0x380] = 3;
        d[0x384] = 7;
        // vertices: (0,0,0) (1,0,0) (1,1,0) (0,1,0) (0,0,5)
        let verts: [[i16; 3]; 5] = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 5]];
        for (i, v) in verts.iter().enumerate() {
            for (k, c) in v.iter().enumerate() {
                d[0x300 + i * 6 + k * 2..0x300 + i * 6 + k * 2 + 2].copy_from_slice(&c.to_le_bytes());
            }
        }
        // quad 0-1-2-3, material 0, normal +z
        put(&mut d, 0x200 + 8, 1.0f32.to_bits());
        for (k, i) in [0u16, 1, 2, 3].iter().enumerate() {
            d[0x200 + 16 + k * 2..0x200 + 18 + k * 2].copy_from_slice(&i.to_le_bytes());
        }
        // triangle 1-2-4 (fourth index 0 is off its plane), material 1
        put(&mut d, 0x220, 1.0f32.to_bits());
        d[0x220 + 12] = 1;
        for (k, i) in [1u16, 2, 4, 0].iter().enumerate() {
            d[0x220 + 16 + k * 2..0x220 + 18 + k * 2].copy_from_slice(&i.to_le_bytes());
        }
        Resource { version: 32, system: d, graphics: Vec::new() }
    }

    #[test]
    fn curved_bounds_become_cylinders_along_their_short_side() {
        let s = 0x5000_0000u32;
        let mut d = vec![0u8; 0x200];
        put(&mut d, 0x08, s + 0x40);
        let b = 0x40;
        d[b + 4] = kind::CURVED;
        for (o, v) in [(0x10, [0.1f32, 0.4, 0.4]), (0x20, [-0.1, -0.4, -0.4])] {
            for (k, c) in v.iter().enumerate() {
                put(&mut d, b + o + k * 4, c.to_bits());
            }
        }
        let (dr, stats) = parse_wbn(&Resource { version: 32, system: d, graphics: Vec::new() }).unwrap();
        assert_eq!(stats.primitives, 1);
        let (lo, hi) = (dr.bounds_min, dr.bounds_max);
        assert!((hi[0] - 0.1).abs() < 1e-4 && (lo[0] + 0.1).abs() < 1e-4, "{lo:?} {hi:?}");
        assert!((hi[1] - 0.4).abs() < 0.02 && (hi[2] - 0.4).abs() < 0.02, "{hi:?}");
    }

    #[test]
    fn reads_quads_and_triangles() {
        let (d, stats) = parse_wbn(&wbn()).unwrap();
        assert_eq!(stats.triangles, 3);
        assert_eq!(stats.materials, vec![3, 7]);
        let hashes: Vec<u32> = d.shaders.iter().map(|s| s.name_hash).collect();
        assert_eq!(hashes, vec![MATERIAL_BASE + 3, MATERIAL_BASE + 7]);
    }
}
