//! Collision bounds (`.ybn`, `phBound`), turned into meshes for display.
//!
//! Every bound starts with (after the 16-byte resource header) its type at
//! `0x10`, sphere radius `0x14`, box max `0x20`, margin `0x2C`, box min `0x30`,
//! box centre `0x40`, sphere centre `0x50`. Types: 0 sphere, 1 capsule, 3 box,
//! 4 geometry, 8 geometry with BVH, 10 composite, 12 disc, 13 cylinder.
//!
//! * Composite: children `0x70`, transforms `0x78` (4×4, column-major), count `0xA0`.
//! * Geometry: polygons `0x88` (16 bytes each, type in the low 3 bits of the
//!   first byte), quantum `0x90`, centre `0xA0`, vertices `0xB0` (3 × i16,
//!   × quantum + centre), vertex count `0xD0`, polygon count `0xD4`, materials
//!   `0xF0` (8 bytes, type in the low byte), per-polygon material index `0x118`.
//! * Polygon types: 0 triangle (u16 vertex indices at 4, 6, 8; top bit is a
//!   flag), 1 sphere (vertex 2, radius 4), 2 capsule and 4 cylinder (vertices
//!   2 and 8, radius 4), 3 box (4 alternating corners at 4, 6, 8, 10).

use rage_resource::{Resource, ResourceError};

use crate::drawable::{Drawable, Mesh, Shader};
use crate::geometry::{Builder, add, scale, sub};
use crate::math::{self, Mat4};

pub mod kind {
    pub const SPHERE: u8 = 0;
    pub const CAPSULE: u8 = 1;
    pub const BOX: u8 = 3;
    pub const GEOMETRY: u8 = 4;
    pub const BVH: u8 = 8;
    pub const COMPOSITE: u8 = 10;
    pub const DISC: u8 = 12;
    pub const CYLINDER: u8 = 13;
}

/// Shader name hashes of collision materials: `joaat("collision")` + the material type.
pub const MATERIAL_BASE: u32 = 0xC011_0000;

/// What a collision file contains.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub bounds: usize,
    pub triangles: usize,
    /// Spheres, capsules, boxes and cylinders (as polygons or whole bounds).
    pub primitives: usize,
    pub vertices: usize,
    /// Material types used (indices into the game's `materials.dat`).
    pub materials: Vec<u8>,
}

fn vec3(res: &Resource, at: u64) -> Result<[f32; 3], ResourceError> {
    let b = res.bytes(at, 12)?;
    Ok([0, 4, 8].map(|o| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])))
}

fn f32_at(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    Ok(f32::from_bits(res.u32(at)?))
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
        return Err(ResourceError::Format("bounds nested too deeply".into()));
    }
    stats.bounds += 1;
    let kind = res.u8(at + 0x10)?;
    let material = res.u8(at + 0x4C)?;
    match kind {
        kind::COMPOSITE => {
            let children = res.u64(at + 0x70)?;
            let transforms = res.u64(at + 0x78)?;
            let count = res.u16(at + 0xA0)? as u64;
            for i in 0..count.min(4096) {
                let child = res.u64(children + i * 8)?;
                if child == 0 {
                    continue;
                }
                let mut t = math::IDENTITY;
                if transforms != 0 {
                    let raw = res.bytes(transforms + i * 64, 64)?;
                    for (k, c) in raw.as_chunks::<4>().0.iter().enumerate() {
                        t[k] = f32::from_le_bytes(*c);
                    }
                    // The w components hold other data.
                    (t[3], t[7], t[11], t[15]) = (0.0, 0.0, 0.0, 1.0);
                }
                bound(res, child, &math::mul(m, &t), b, stats, depth + 1)?;
            }
        }
        kind::GEOMETRY | kind::BVH => geometry(res, at, m, b, stats)?,
        kind::SPHERE => {
            let c = vec3(res, at + 0x50)?;
            b.round(material, m, c, c, f32_at(res, at + 0x14)?, true);
        }
        kind::BOX => {
            let (lo, hi) = (vec3(res, at + 0x30)?, vec3(res, at + 0x20)?);
            let c = scale(add(lo, hi), 0.5);
            let h = scale(sub(hi, lo), 0.5);
            b.cuboid(material, m, c, [[h[0], 0.0, 0.0], [0.0, h[1], 0.0], [0.0, 0.0, h[2]]]);
        }
        kind::CAPSULE => {
            let c = vec3(res, at + 0x40)?;
            let margin = f32_at(res, at + 0x2C)?;
            let half = f32_at(res, at + 0x14)? - margin;
            b.round(material, m, sub(c, [0.0, half, 0.0]), add(c, [0.0, half, 0.0]), margin, true);
        }
        kind::CYLINDER | kind::DISC => {
            let (lo, hi) = (vec3(res, at + 0x30)?, vec3(res, at + 0x20)?);
            let c = scale(add(lo, hi), 0.5);
            let (r, half) = if kind == kind::DISC {
                (f32_at(res, at + 0x14)?, f32_at(res, at + 0x2C)?)
            } else {
                ((hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0)
            };
            // Cylinders stand along Y, discs lie along X.
            let axis = if kind == kind::DISC { [half, 0.0, 0.0] } else { [0.0, half, 0.0] };
            b.round(material, m, sub(c, axis), add(c, axis), r, false);
        }
        _ => {}
    }
    Ok(())
}

fn geometry(res: &Resource, at: u64, m: &Mat4, b: &mut Builder, stats: &mut Stats) -> Result<(), ResourceError> {
    let polys = res.u64(at + 0x88)?;
    let quantum = vec3(res, at + 0x90)?;
    let centre = vec3(res, at + 0xA0)?;
    let verts_at = res.u64(at + 0xB0)?;
    let vert_count = res.u32(at + 0xD0)? as u64;
    let poly_count = res.u32(at + 0xD4)? as u64;
    let materials_at = res.u64(at + 0xF0)?;
    let material_count = res.u8(at + 0x120)? as u64;
    let poly_materials = res.u64(at + 0x118)?;
    if vert_count > 1 << 20 || poly_count > 1 << 21 {
        return Err(ResourceError::Format(format!(
            "implausible geometry: {vert_count} vertices, {poly_count} polygons"
        )));
    }
    stats.vertices += vert_count as usize;
    let raw = if vert_count > 0 { res.bytes(verts_at, vert_count as usize * 6)? } else { &[][..] };
    let verts: Vec<[f32; 3]> = raw
        .as_chunks::<6>()
        .0
        .iter()
        .map(|c| {
            let q = [0, 2, 4].map(|o| i16::from_le_bytes([c[o], c[o + 1]]) as f32);
            [q[0] * quantum[0] + centre[0], q[1] * quantum[1] + centre[1], q[2] * quantum[2] + centre[2]]
        })
        .collect();
    let types: Vec<u8> = (0..material_count.max(1))
        .map(|i| if materials_at != 0 { res.u8(materials_at + i * 8).unwrap_or(0) } else { 0 })
        .collect();
    let data = if poly_count > 0 { res.bytes(polys, poly_count as usize * 16)? } else { &[][..] };
    let index_bytes =
        if poly_materials != 0 && poly_count > 0 { res.bytes(poly_materials, poly_count as usize)? } else { &[][..] };
    let vert = |i: u16| verts.get((i & 0x7FFF) as usize).copied();
    let u16_at = |p: &[u8], o: usize| u16::from_le_bytes([p[o], p[o + 1]]);
    let f32_in = |p: &[u8], o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
    for (i, p) in data.as_chunks::<16>().0.iter().enumerate() {
        let material = types.get(*index_bytes.get(i).unwrap_or(&0) as usize).copied().unwrap_or(0);
        match p[0] & 7 {
            0 => {
                if let (Some(a), Some(c), Some(d)) = (vert(u16_at(p, 4)), vert(u16_at(p, 6)), vert(u16_at(p, 8))) {
                    b.triangle(material, m, [a, c, d]);
                    stats.triangles += 1;
                }
            }
            1 => {
                if let Some(c) = vert(u16_at(p, 2)) {
                    b.round(material, m, c, c, f32_in(p, 4), true);
                }
            }
            2 | 4 => {
                if let (Some(a), Some(c)) = (vert(u16_at(p, 2)), vert(u16_at(p, 8))) {
                    b.round(material, m, a, c, f32_in(p, 4), p[0] & 7 == 2);
                }
            }
            3 => {
                let corners = [4, 6, 8, 10].map(|o| vert(u16_at(p, o)));
                if let [Some(v1), Some(v2), Some(v3), Some(v4)] = corners {
                    // Four alternating corners of the box: the sum of two of
                    // them, seen from the centre, is twice a half axis.
                    let c = scale(add(add(v1, v2), add(v3, v4)), 0.25);
                    let s = [v1, v2, v3, v4].map(|v| sub(v, c));
                    let axes = [scale(add(s[0], s[1]), 0.5), scale(add(s[0], s[2]), 0.5), scale(add(s[0], s[3]), 0.5)];
                    b.cuboid(material, m, c, axes);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Reads a collision file and turns it into a drawable (one mesh per
/// material type; shader name hash = [`MATERIAL_BASE`] + material type).
pub fn parse(res: &Resource) -> Result<(Drawable, Stats), ResourceError> {
    parse_at(res, Resource::ROOT)
}

/// Reads bounds located at `at` (e.g. those embedded in a model).
pub fn parse_at(res: &Resource, at: u64) -> Result<(Drawable, Stats), ResourceError> {
    let mut b = Builder::default();
    let mut stats = Stats::default();
    bound(res, at, &math::IDENTITY, &mut b, &mut stats, 0)?;
    Ok(finish(b, stats))
}

/// Turns built collision meshes into a drawable: one mesh per material type,
/// shader name hash [`MATERIAL_BASE`] + material type.
pub(crate) fn finish(b: Builder, mut stats: Stats) -> (Drawable, Stats) {
    let mut d = Drawable { name: "collision".into(), ..Default::default() };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    stats.materials = b.meshes.iter().map(|(m, _)| *m).collect();
    stats.materials.sort_unstable();
    for (i, (material, mesh)) in b.meshes.into_iter().enumerate() {
        for p in &mesh.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        d.shaders.push(Shader { name_hash: MATERIAL_BASE + material as u32, ..Default::default() });
        d.meshes.push(Mesh { shader: i as u16, ..mesh });
    }
    if lo[0] <= hi[0] {
        (d.bounds_min, d.bounds_max) = (lo, hi);
    }
    stats.primitives = b.primitives;
    (d, stats)
}

/// Names of the material types, from the game's `materials.dat` (one material
/// per line, in index order).
pub fn material_names(materials_dat: &str) -> Vec<String> {
    materials_dat
        .lines()
        .skip(1)
        .filter(|l| l.len() >= 20 && !l.starts_with('#'))
        .filter_map(|l| {
            let parts: Vec<&str> = l.split([' ', '\t']).filter(|p| !p.is_empty()).collect();
            (parts.len() >= 10).then(|| parts[0].to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_material_names() {
        let dat = "12.00\n# comment\nDEFAULT\t\t\tDefault\t\tVOID\tNONE\tNONE\tNONE\t0.5\t0.1\t1.0\t1.0\n\
                   CONCRETE\t\tConcrete\tCONCRETE\tNONE\tNONE\tNONE\t0.7\t0.1\t2.0\t1.0\n";
        assert_eq!(material_names(dat), vec!["DEFAULT", "CONCRETE"]);
    }
}
