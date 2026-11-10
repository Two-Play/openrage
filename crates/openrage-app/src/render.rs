//! A small software renderer for model previews (Slint has no 3D view).
//!
//! Perspective orbit camera, z-buffer, perspective-correct texture mapping
//! with alpha cut-out, and simple headlight shading. Fast enough for
//! interactive previews of game models (tens of thousands of triangles).

use std::collections::HashMap;

use rage_formats::math;

use crate::models::LoadedModel;

/// Camera angles in radians and zoom (1 = whole model in view).
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self { yaw: -2.3, pitch: 0.35, zoom: 1.15 }
    }
}

struct Image {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

impl Image {
    fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        let x = ((u - u.floor()) * self.w as f32) as usize % self.w;
        let y = ((v - v.floor()) * self.h as f32) as usize % self.h;
        let i = (y * self.w + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// Decoded base textures, shared between frames.
pub struct Materials {
    images: Vec<Image>,
    /// Index into `images` per drawable and shader.
    per_shader: Vec<Vec<Option<usize>>>,
}

impl Materials {
    /// Decodes the base texture of every shader of every drawable.
    pub fn new(model: &LoadedModel) -> Self {
        let mut by_name: HashMap<String, Option<usize>> = HashMap::new();
        let mut images: Vec<Image> = Vec::new();
        let per_shader = model
            .drawables
            .iter()
            .map(|(_, d)| {
                d.shaders
                    .iter()
                    .map(|s| {
                        s.base_texture().and_then(|name| {
                            *by_name.entry(name.to_ascii_lowercase()).or_insert_with(|| {
                                let t = model.texture(d, name)?;
                                let rgba = t.to_rgba()?;
                                images.push(Image { w: t.width.max(1) as usize, h: t.height.max(1) as usize, rgba });
                                Some(images.len() - 1)
                            })
                        })
                    })
                    .collect()
            })
            .collect();
        Self { images, per_shader }
    }

    fn image(&self, drawable: usize, shader: u16) -> Option<&Image> {
        self.per_shader.get(drawable)?.get(shader as usize).copied().flatten().map(|i| &self.images[i])
    }

    /// RGBA pixels of the base texture of a drawable's shader (for GPU upload).
    pub fn texture(&self, drawable: usize, shader: u16) -> Option<(usize, usize, usize, &[u8])> {
        let i = self.per_shader.get(drawable)?.get(shader as usize).copied().flatten()?;
        let img = &self.images[i];
        Some((i, img.w, img.h, &img.rgba))
    }
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// Model-space bounds of the shown parts, `None` if there is nothing to show.
pub fn bounds(model: &LoadedModel, visible: &[bool]) -> Option<([f32; 3], [f32; 3])> {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (part, d) in model.shown(visible) {
        for p in d.meshes.iter().flat_map(|m| &m.positions) {
            let p = math::transform_point(&part.transform, *p);
            for i in 0..3 {
                if p[i].is_finite() {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

/// Camera for `view` looking at the visible parts: eye position, forward,
/// right and up vectors, distance and vertical field of view.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub distance: f32,
    pub radius: f32,
    pub fov: f32,
}

pub fn orbit_camera(lo: [f32; 3], hi: [f32; 3], view: View) -> Camera {
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let radius = (dot(sub(hi, lo), sub(hi, lo)).sqrt() / 2.0).max(1e-3);
    let fov = 40f32.to_radians();
    let distance = radius / (fov / 2.0).sin() / view.zoom.max(0.05);
    let (cp, sp, cy, sy) = (view.pitch.cos(), view.pitch.sin(), view.yaw.cos(), view.yaw.sin());
    let eye = [center[0] + distance * cp * cy, center[1] + distance * cp * sy, center[2] + distance * sp];
    let forward = norm(sub(center, eye));
    let right = norm(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    Camera { eye, forward, right, up, distance, radius, fov }
}

/// Renders the parts of `model` shown by `visible` into an RGBA image of
/// `width × height` with a transparent background.
pub fn render(
    model: &LoadedModel,
    visible: &[bool],
    materials: &Materials,
    view: View,
    width: usize,
    height: usize,
) -> Vec<u8> {
    // Bounds from the vertices (more reliable than stored bounds).
    match bounds(model, visible) {
        Some((lo, hi)) => render_camera(model, visible, materials, &orbit_camera(lo, hi, view), width, height),
        None => vec![0u8; width.max(1) * height.max(1) * 4],
    }
}

/// First-person camera at `eye` looking along `yaw`/`pitch` (radians);
/// `radius` is the model's size, for the clipping planes.
pub fn fly_camera(eye: [f32; 3], yaw: f32, pitch: f32, radius: f32) -> Camera {
    let (cp, sp, cy, sy) = (pitch.cos(), pitch.sin(), yaw.cos(), yaw.sin());
    let forward = norm([cp * cy, cp * sy, sp]);
    let right = norm(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    Camera { eye, forward, right, up, distance: radius.max(0.01), radius: radius.max(0.01), fov: 60f32.to_radians() }
}

/// Renders from any camera (see [`orbit_camera`], [`fly_camera`]).
pub fn render_camera(
    model: &LoadedModel,
    visible: &[bool],
    materials: &Materials,
    camera: &Camera,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let (w, h) = (width.max(1), height.max(1));
    // Transparent background: the UI (or the PNG viewer) shows through.
    let mut color = vec![0u8; w * h * 4];
    let mut depth = vec![f32::INFINITY; w * h];
    let Camera { eye, forward, right, up, distance, fov, .. } = *camera;
    // Fit the field of view to the shorter side so tall previews do not crop.
    let focal = (w.min(h) as f32 / 2.0) / (fov / 2.0).tan();
    let near = distance * 0.01;

    // Camera space → (screen x, screen y, view depth).
    let project = |p: V3| -> (f32, f32, f32) {
        let r = sub(p, eye);
        let z = dot(r, forward);
        (w as f32 / 2.0 + dot(r, right) * focal / z, h as f32 / 2.0 - dot(r, up) * focal / z, z)
    };

    for (part, d) in model.shown(visible) {
        for mesh in &d.meshes {
            let img = materials.image(part.drawable, mesh.shader);
            let has_uv = mesh.uvs.len() == mesh.positions.len();
            let has_n = mesh.normals.len() == mesh.positions.len();
            let positions: Vec<[f32; 3]> =
                mesh.positions.iter().map(|p| math::transform_point(&part.transform, *p)).collect();
            let normals: Vec<[f32; 3]> =
                mesh.normals.iter().map(|n| math::transform_dir(&part.transform, *n)).collect();
            let pts: Vec<(f32, f32, f32)> = positions.iter().map(|p| project(*p)).collect();
            for tri in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                let (pa, pb, pc) = (pts[a], pts[b], pts[c]);
                if pa.2 < near || pb.2 < near || pc.2 < near {
                    continue;
                }
                let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
                if area.abs() < 1e-9 {
                    continue;
                }
                // Face normal for shading when vertex normals are missing.
                let face_n = norm(cross(sub(positions[b], positions[a]), sub(positions[c], positions[a])));
                let minx = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as usize;
                let maxx = pa.0.max(pb.0).max(pc.0).ceil().min(w as f32 - 1.0);
                let miny = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as usize;
                let maxy = pa.1.max(pb.1).max(pc.1).ceil().min(h as f32 - 1.0);
                if maxx < 0.0 || maxy < 0.0 {
                    continue;
                }
                let (iza, izb, izc) = (1.0 / pa.2, 1.0 / pb.2, 1.0 / pc.2);
                for y in miny..=maxy as usize {
                    for x in minx..=maxx as usize {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((pb.0 - px) * (pc.1 - py) - (pb.1 - py) * (pc.0 - px)) / area;
                        let w1 = ((pc.0 - px) * (pa.1 - py) - (pc.1 - py) * (pa.0 - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let iz = w0 * iza + w1 * izb + w2 * izc;
                        let z = 1.0 / iz;
                        let di = y * w + x;
                        if z >= depth[di] {
                            continue;
                        }
                        // Perspective-correct weights.
                        let (q0, q1, q2) = (w0 * iza * z, w1 * izb * z, w2 * izc * z);
                        let mut base = [0.72f32, 0.72, 0.74, 1.0];
                        if let (Some(img), true) = (img, has_uv) {
                            let u = q0 * mesh.uvs[a][0] + q1 * mesh.uvs[b][0] + q2 * mesh.uvs[c][0];
                            let v = q0 * mesh.uvs[a][1] + q1 * mesh.uvs[b][1] + q2 * mesh.uvs[c][1];
                            let t = img.sample(u, v);
                            if t[3] < 77 {
                                continue; // alpha cut-out
                            }
                            base = [t[0] as f32 / 255.0, t[1] as f32 / 255.0, t[2] as f32 / 255.0, 1.0];
                        }
                        let n = if has_n {
                            let (na, nb, nc) = (normals[a], normals[b], normals[c]);
                            norm([
                                q0 * na[0] + q1 * nb[0] + q2 * nc[0],
                                q0 * na[1] + q1 * nb[1] + q2 * nc[1],
                                q0 * na[2] + q1 * nb[2] + q2 * nc[2],
                            ])
                        } else {
                            face_n
                        };
                        // Headlight plus a little sky light; double-sided.
                        let light = 0.25 + 0.65 * dot(n, forward).abs() + 0.1 * n[2].max(0.0);
                        depth[di] = z;
                        let o = &mut color[di * 4..di * 4 + 4];
                        for k in 0..3 {
                            o[k] = (base[k] * light * 255.0).clamp(0.0, 255.0) as u8;
                        }
                        o[3] = 255;
                    }
                }
            }
        }
    }
    color
}
