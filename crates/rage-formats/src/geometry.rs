//! Building display meshes from shapes: triangles with flat normals, boxes,
//! spheres, capsules, cylinders, and markers and bars for points and lines.

use crate::drawable::Mesh;
use crate::math::{self, Mat4};

/// Collects triangles per material.
#[derive(Default)]
pub struct Builder {
    pub meshes: Vec<(u8, Mesh)>,
    /// Spheres, capsules, boxes and cylinders added.
    pub primitives: usize,
}

impl Builder {
    pub fn mesh(&mut self, material: u8) -> &mut Mesh {
        let i = match self.meshes.iter().position(|(m, _)| *m == material) {
            Some(i) => i,
            None => {
                self.meshes.push((material, Mesh::default()));
                self.meshes.len() - 1
            }
        };
        &mut self.meshes[i].1
    }

    /// Adds a triangle with a flat normal.
    pub fn triangle(&mut self, material: u8, m: &Mat4, p: [[f32; 3]; 3]) {
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

    pub fn quad(&mut self, material: u8, m: &Mat4, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        self.triangle(material, m, [a, b, c]);
        self.triangle(material, m, [a, c, d]);
    }

    /// A box from its centre and three half axes.
    pub fn cuboid(&mut self, material: u8, m: &Mat4, c: [f32; 3], ax: [[f32; 3]; 3]) {
        self.primitives += 1;
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
    pub fn round(&mut self, material: u8, m: &Mat4, a: [f32; 3], b: [f32; 3], r: f32, caps: bool) {
        self.primitives += 1;
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

pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    scale(a, 1.0 / l)
}

impl Builder {
    /// A diamond (octahedron) marking a point.
    pub fn marker(&mut self, material: u8, p: [f32; 3], size: f32) {
        let m = math::IDENTITY;
        let d = [[size, 0.0, 0.0], [0.0, size, 0.0], [-size, 0.0, 0.0], [0.0, -size, 0.0]];
        let (top, bottom) = (add(p, [0.0, 0.0, size * 1.4]), add(p, [0.0, 0.0, -size * 1.4]));
        for i in 0..4 {
            let (a, b) = (add(p, d[i]), add(p, d[(i + 1) % 4]));
            self.triangle(material, &m, [a, b, top]);
            self.triangle(material, &m, [b, a, bottom]);
        }
    }

    /// A flat-topped bar from `a` to `b` (for links and paths).
    pub fn bar(&mut self, material: u8, a: [f32; 3], b: [f32; 3], width: f32) {
        let dir = sub(b, a);
        let len = dot(dir, dir).sqrt();
        if len.is_nan() || len <= 1e-4 {
            return;
        }
        let f = scale(dir, 0.5);
        let side = cross(dir, [0.0, 0.0, 1.0]);
        let side = if dot(side, side) < 1e-8 { [width / 2.0, 0.0, 0.0] } else { scale(normalize(side), width / 2.0) };
        let up = scale(normalize(cross(side, dir)), width / 4.0);
        let primitives = self.primitives;
        self.cuboid(material, &math::IDENTITY, scale(add(a, b), 0.5), [f, side, up]);
        self.primitives = primitives;
    }
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
        assert_eq!(b.primitives, 3);
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
    fn markers_and_bars() {
        let mut b = Builder::default();
        b.marker(0, [1.0, 2.0, 3.0], 0.5);
        b.bar(1, [0.0; 3], [10.0, 0.0, 0.0], 0.2);
        b.bar(1, [0.0; 3], [0.0, 0.0, 5.0], 0.2);
        assert_eq!(b.meshes[0].1.indices.len(), 24);
        assert_eq!(b.meshes[1].1.indices.len(), 72);
        assert_eq!(b.primitives, 0);
    }
}
