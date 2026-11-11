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

/// Collects triangles per material type.
#[derive(Default)]
struct Builder {
    meshes: Vec<(u8, Mesh)>,
    stats: Stats,
}

impl Builder {
    fn mesh(&mut self, material: u8) -> &mut Mesh {
        let i = match self.meshes.iter().position(|(m, _)| *m == material) {
            Some(i) => i,
            None => {
                self.meshes.push((material, Mesh::default()));
                if !self.stats.materials.contains(&material) {
                    self.stats.materials.push(material);
                }
                self.meshes.len() - 1
            }
        };
        &mut self.meshes[i].1
    }

    /// Adds a triangle with a flat normal.
    fn triangle(&mut self, material: u8, m: &Mat4, p: [[f32; 3]; 3]) {
        let p = p.map(|v| math::transform_point(m, v));
        let (u, v) = (sub(p[1], p[0]), sub(p[2], p[0]));
        let n = normalize(cross(u, v));
        if !n.iter().all(|c| c.is_finite()) {
            return;
        }
        let mesh = self.mesh(material);
        let base = mesh.positions.len() as u32;
        for q in p {
            mesh.positions.push(q);
            mesh.normals.push(n);
        }
        mesh.indices.extend([base, base + 1, base + 2]);
    }

    fn quad(&mut self, material: u8, m: &Mat4, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        self.triangle(material, m, [a, b, c]);
        self.triangle(material, m, [a, c, d]);
    }

    /// A box from its centre and three half axes.
    fn cuboid(&mut self, material: u8, m: &Mat4, c: [f32; 3], ax: [[f32; 3]; 3]) {
        self.stats.primitives += 1;
        let corner = |sx: f32, sy: f32, sz: f32| add(c, add(scale(ax[0], sx), add(scale(ax[1], sy), scale(ax[2], sz))));
        let v = [
            corner(-1.0, -1.0, -1.0),
            corner(1.0, -1.0, -1.0),
            corner(1.0, 1.0, -1.0),
            corner(-1.0, 1.0, -1.0),
            corner(-1.0, -1.0, 1.0),
            corner(1.0, -1.0, 1.0),
            corner(1.0, 1.0, 1.0),
            corner(-1.0, 1.0, 1.0),
        ];
        // Faces wound outwards for a right-handed set of axes; flip otherwise.
        let flip = dot(cross(ax[0], ax[1]), ax[2]) < 0.0;
        let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3]];
        for f in faces {
            let [a, b, c2, d] = f.map(|i| v[i]);
            if flip { self.quad(material, m, a, d, c2, b) } else { self.quad(material, m, a, b, c2, d) }
        }
    }

    /// A sphere (`a == b`), capsule or cylinder between two points.
    fn round(&mut self, material: u8, m: &Mat4, a: [f32; 3], b: [f32; 3], r: f32, caps: bool) {
        self.stats.primitives += 1;
        if !(r.is_finite() && r > 0.0) {
            return;
        }
        const SEG: usize = 16;
        const RINGS: usize = 6;
        let axis = sub(b, a);
        let len = dot(axis, axis).sqrt();
        let w = if len > 1e-6 { scale(axis, 1.0 / len) } else { [0.0, 0.0, 1.0] };
        let helper = if w[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        let u = normalize(cross(helper, w));
        let v = cross(w, u);
        let at = |centre: [f32; 3], theta: f32, phi: f32| {
            // phi: angle from the equator towards +w.
            let (s, c) = theta.sin_cos();
            let ring = add(scale(u, c * phi.cos() * r), scale(v, s * phi.cos() * r));
            add(centre, add(ring, scale(w, phi.sin() * r)))
        };
        let tau = std::f32::consts::TAU;
        let half = std::f32::consts::FRAC_PI_2;
        for i in 0..SEG {
            let (t0, t1) = (tau * i as f32 / SEG as f32, tau * (i + 1) as f32 / SEG as f32);
            if len > 1e-6 {
                self.quad(material, m, at(a, t0, 0.0), at(a, t1, 0.0), at(b, t1, 0.0), at(b, t0, 0.0));
            }
            if caps {
                // Hemisphere at b (towards +w) and at a (towards -w).
                for k in 0..RINGS {
                    let (p0, p1) = (half * k as f32 / RINGS as f32, half * (k + 1) as f32 / RINGS as f32);
                    self.quad(material, m, at(b, t0, p0), at(b, t1, p0), at(b, t1, p1), at(b, t0, p1));
                    self.quad(material, m, at(a, t0, -p1), at(a, t1, -p1), at(a, t1, -p0), at(a, t0, -p0));
                }
            } else {
                // Flat ends.
                self.triangle(material, m, [b, at(b, t0, 0.0), at(b, t1, 0.0)]);
                self.triangle(material, m, [a, at(a, t1, 0.0), at(a, t0, 0.0)]);
            }
        }
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    scale(a, 1.0 / l)
}

fn vec3(res: &Resource, at: u64) -> Result<[f32; 3], ResourceError> {
    let b = res.bytes(at, 12)?;
    Ok([0, 4, 8].map(|o| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])))
}

fn f32_at(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    Ok(f32::from_bits(res.u32(at)?))
}

fn bound(res: &Resource, at: u64, m: &Mat4, b: &mut Builder, depth: usize) -> Result<(), ResourceError> {
    if depth > 8 {
        return Err(ResourceError::Format("bounds nested too deeply".into()));
    }
    b.stats.bounds += 1;
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
                bound(res, child, &math::mul(m, &t), b, depth + 1)?;
            }
        }
        kind::GEOMETRY | kind::BVH => geometry(res, at, m, b)?,
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

fn geometry(res: &Resource, at: u64, m: &Mat4, b: &mut Builder) -> Result<(), ResourceError> {
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
    b.stats.vertices += vert_count as usize;
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
                    b.stats.triangles += 1;
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
    bound(res, at, &math::IDENTITY, &mut b, 0)?;
    let mut d = Drawable { name: "collision".into(), ..Default::default() };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
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
    b.stats.materials.sort_unstable();
    Ok((d, b.stats))
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
    fn primitives_make_closed_meshes() {
        let mut b = Builder::default();
        b.cuboid(1, &math::IDENTITY, [0.0; 3], [[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]]);
        b.round(2, &math::IDENTITY, [0.0; 3], [0.0, 0.0, 2.0], 0.5, true);
        b.round(2, &math::IDENTITY, [0.0; 3], [0.0; 3], 1.0, true);
        assert_eq!(b.stats.primitives, 3);
        assert_eq!(b.meshes.len(), 2);
        let boxm = &b.meshes[0].1;
        assert_eq!(boxm.indices.len(), 36);
        // Box normals point away from the centre.
        for t in boxm.indices.chunks(3) {
            let c = scale(
                add(add(boxm.positions[t[0] as usize], boxm.positions[t[1] as usize]), boxm.positions[t[2] as usize]),
                1.0 / 3.0,
            );
            assert!(dot(boxm.normals[t[0] as usize], c) > 0.0);
        }
    }

    #[test]
    fn reads_material_names() {
        let dat = "12.00\n# comment\nDEFAULT\t\t\tDefault\t\tVOID\tNONE\tNONE\tNONE\t0.5\t0.1\t1.0\t1.0\n\
                   CONCRETE\t\tConcrete\tCONCRETE\tNONE\tNONE\tNONE\t0.7\t0.1\t2.0\t1.0\n";
        assert_eq!(material_names(dat), vec!["DEFAULT", "CONCRETE"]);
    }
}
