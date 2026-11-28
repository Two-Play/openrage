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

    #[test]
    fn vector_helpers() {
        assert_eq!(add([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), [5.0, 7.0, 9.0]);
        assert_eq!(sub([4.0, 5.0, 6.0], [1.0, 2.0, 3.0]), [3.0, 3.0, 3.0]);
        assert_eq!(scale([1.0, -2.0, 3.0], 2.0), [2.0, -4.0, 6.0]);
        assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(normalize([0.0, 3.0, 4.0]), [0.0, 0.6, 0.8]);
        assert!(normalize([0.0; 3]).iter().all(|c| c.is_nan()));
    }

    #[test]
    fn meshes_are_kept_per_material() {
        let mut b = Builder::default();
        b.mesh(4).positions.push([1.0; 3]);
        b.mesh(2).positions.push([2.0; 3]);
        b.mesh(4).positions.push([3.0; 3]);
        assert_eq!(b.meshes.iter().map(|(m, _)| *m).collect::<Vec<_>>(), [4, 2]);
        assert_eq!(b.meshes[0].1.positions, [[1.0; 3], [3.0; 3]]);
    }

    #[test]
    fn a_triangle_is_transformed_and_gets_a_flat_normal() {
        let mut b = Builder::default();
        let m = math::from_trs([10.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        b.triangle(0, &m, [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        let mesh = &b.meshes[0].1;
        assert_eq!(mesh.positions, [[10.0, 0.0, 0.0], [11.0, 0.0, 0.0], [10.0, 1.0, 0.0]]);
        assert_eq!(mesh.normals, [[0.0, 0.0, 1.0]; 3]);
        assert_eq!(mesh.indices, [0, 1, 2]);
        b.triangle(0, &m, [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(b.meshes[0].1.indices, [0, 1, 2, 3, 4, 5]);
        assert_eq!(b.primitives, 0);
    }

    #[test]
    fn a_degenerate_triangle_adds_nothing() {
        let mut b = Builder::default();
        b.triangle(0, &math::IDENTITY, [[1.0; 3], [1.0; 3], [2.0; 3]]);
        assert!(b.meshes.is_empty());
    }

    #[test]
    fn a_quad_is_two_triangles() {
        let mut b = Builder::default();
        b.quad(1, &math::IDENTITY, [0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        let mesh = &b.meshes[0].1;
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.positions[3..], [[0.0; 3], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn a_box_with_mirrored_axes_still_faces_outwards() {
        let mut b = Builder::default();
        b.cuboid(0, &math::IDENTITY, [0.0; 3], [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let mesh = &b.meshes[0].1;
        assert_eq!(mesh.indices.len(), 36);
        for t in mesh.indices.chunks(3) {
            let a = mesh.positions[t[0] as usize];
            assert!(dot(mesh.normals[t[0] as usize], a) > 0.0);
        }
    }

    #[test]
    fn cylinders_have_flat_ends_and_capsules_round_ones() {
        let (a, c) = ([0.0; 3], [0.0, 0.0, 4.0]);
        let mut b = Builder::default();
        b.round(0, &math::IDENTITY, a, c, 1.0, false);
        assert_eq!(b.meshes[0].1.indices.len(), 16 * (2 + 2) * 3);
        let mut b = Builder::default();
        b.round(0, &math::IDENTITY, a, c, 1.0, true);
        let n = b.meshes[0].1.indices.len();
        assert!(n > 16 * 24 * 3 && n <= 16 * (2 + 24) * 3, "{n}");
        let mesh = &b.meshes[0].1;
        let top = mesh.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
        let bottom = mesh.positions.iter().map(|p| p[2]).fold(f32::MAX, f32::min);
        assert!((top - 5.0).abs() < 1e-5 && (bottom + 1.0).abs() < 1e-5, "{top} {bottom}");
    }

    #[test]
    fn a_sphere_has_no_side() {
        let mut b = Builder::default();
        b.round(0, &math::IDENTITY, [1.0; 3], [1.0; 3], 2.0, true);
        let mesh = &b.meshes[0].1;
        assert!(mesh.positions.iter().all(|p| (dot(sub(*p, [1.0; 3]), sub(*p, [1.0; 3])).sqrt() - 2.0).abs() < 1e-4));
        let top = mesh.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
        assert!((top - 3.0).abs() < 1e-4, "{top}");
        assert!(mesh.indices.len() > 16 * 22 * 3 && mesh.indices.len() <= 16 * 24 * 3);
        assert_eq!(b.primitives, 1);
    }

    #[test]
    fn a_round_shape_without_a_radius_counts_but_draws_nothing() {
        let mut b = Builder::default();
        for r in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            b.round(0, &math::IDENTITY, [0.0; 3], [0.0, 1.0, 0.0], r, true);
        }
        assert_eq!(b.primitives, 4);
        assert!(b.meshes.is_empty());
    }

    #[test]
    fn a_round_shape_along_x_or_z_gets_a_perpendicular_frame() {
        for end in [[3.0, 0.0, 0.0], [0.0, 0.0, 3.0], [0.0, 3.0, 0.0]] {
            let mut b = Builder::default();
            b.round(0, &math::IDENTITY, [0.0; 3], end, 1.0, false);
            let mesh = &b.meshes[0].1;
            assert!(mesh.normals.iter().all(|n| n.iter().all(|c| c.is_finite())), "{end:?}");
            let axis = normalize(end);
            let radial = mesh.positions.iter().map(|p| sub(*p, scale(axis, dot(*p, axis)))).map(|q| dot(q, q).sqrt());
            assert!(radial.into_iter().all(|r| r < 1.0 + 1e-5), "{end:?}");
        }
    }

    #[test]
    fn a_bar_between_equal_or_broken_points_is_skipped() {
        let mut b = Builder::default();
        b.bar(0, [1.0; 3], [1.0; 3], 0.5);
        b.bar(0, [0.0; 3], [f32::NAN, 0.0, 0.0], 0.5);
        assert!(b.meshes.is_empty());
    }

    #[test]
    fn a_vertical_bar_has_a_side_too() {
        let mut b = Builder::default();
        b.bar(0, [0.0; 3], [0.0, 0.0, 2.0], 1.0);
        let mesh = &b.meshes[0].1;
        assert_eq!(mesh.indices.len(), 36);
        let (lo, hi) = mesh.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k])))
        });
        assert!((hi[2] - 2.0).abs() < 1e-5 && lo[2].abs() < 1e-5);
        assert!((hi[0] - 0.5).abs() < 1e-5 && (lo[0] + 0.5).abs() < 1e-5, "{lo:?} {hi:?}");
    }
}
