//! A small software renderer for model previews (Slint has no 3D view).
//!
//! Perspective orbit camera, z-buffer, perspective-correct texture mapping
//! with alpha cut-out, and simple headlight shading. Fast enough for
//! interactive previews of game models (tens of thousands of triangles).

use rage_formats::drawable::param;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};

use rage_formats::math;
use rage_formats::texture::Texture;

use crate::models::{LoadedModel, Look};

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

/// A picture on a model, shared by those with the same one.
pub struct Image {
    w: usize,
    h: usize,
    /// Decoded on first use.
    rgba: OnceLock<Option<Vec<u8>>>,
    /// As the game has it; let go once it is on the graphics card.
    source: Mutex<Option<Arc<Texture>>>,
}

impl Image {
    fn decoded(w: usize, h: usize, rgba: Vec<u8>) -> Self {
        Self { w, h, rgba: OnceLock::from(Some(rgba)), source: Mutex::new(None) }
    }

    /// Width and height.
    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// RGBA pixels.
    pub fn rgba(&self) -> Option<&[u8]> {
        let decode = || self.source().and_then(|t| t.to_rgba());
        self.rgba.get_or_init(decode).as_deref()
    }

    /// The texture as the game has it, if it is kept.
    pub fn source(&self) -> Option<Arc<Texture>> {
        self.source.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Lets go of it: it is elsewhere now.
    pub fn release(&self) {
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The picture of a texture that several models share.
    fn shared(texture: &Arc<Texture>) -> Arc<Image> {
        type Known = HashMap<usize, (Weak<Texture>, Weak<Image>)>;
        static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Default::default);
        let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
        let key = Arc::as_ptr(texture) as usize;
        if let Some((of, image)) = known.get(&key)
            && of.upgrade().is_some_and(|of| Arc::ptr_eq(&of, texture))
            && let Some(image) = image.upgrade()
        {
            return image;
        }
        let image = Arc::new(Image {
            w: texture.width.max(1) as usize,
            h: texture.height.max(1) as usize,
            rgba: OnceLock::new(),
            source: Mutex::new(Some(Arc::clone(texture))),
        });
        if known.len().is_power_of_two() {
            known.retain(|_, (_, image)| image.strong_count() > 0);
        }
        known.insert(key, (Arc::downgrade(texture), Arc::downgrade(&image)));
        image
    }

    fn sample(&self, rgba: &[u8], u: f32, v: f32) -> [u8; 4] {
        let x = ((u - u.floor()) * self.w as f32) as usize % self.w;
        let y = ((v - v.floor()) * self.h as f32) as usize % self.h;
        let i = (y * self.w + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }
}

/// The textures of a model, ready to be drawn.
pub struct Materials {
    images: Vec<Arc<Image>>,
    /// Indices into `images` per drawable and shader.
    per_shader: Vec<Vec<Slots>>,
}

/// A shader's textures: indices into [`Materials::images`].
#[derive(Clone, Copy, Default)]
struct Slots {
    base: Option<usize>,
    /// Drawn over the base on the second UV set (liveries, signs).
    overlay: Option<usize>,
    normal: Option<usize>,
    specular: Option<usize>,
}

impl Materials {
    /// Decodes the textures of every shader of every drawable.
    pub fn new(model: &LoadedModel) -> Self {
        let mut by_name: HashMap<String, Option<usize>> = HashMap::new();
        let mut images: Vec<Arc<Image>> = Vec::new();
        // `opaque`: the shader's alpha channel holds other data (skin shaders).
        let mut decode = |d: &rage_formats::drawable::Drawable, name: Option<&str>, opaque: bool| {
            let name = name?;
            let key = format!("{}{}", name.to_ascii_lowercase(), if opaque { "#opaque" } else { "" });
            *by_name.entry(key).or_insert_with(|| {
                let t = model.texture(d, name)?;
                if let Some(shared) = model.shared_texture(name).filter(|s| !opaque && std::ptr::eq(&***s, t)) {
                    images.push(Image::shared(shared));
                    return Some(images.len() - 1);
                }
                let mut rgba = t.to_rgba()?;
                if opaque {
                    rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
                }
                images.push(Arc::new(Image::decoded(t.width.max(1) as usize, t.height.max(1) as usize, rgba)));
                Some(images.len() - 1)
            })
        };
        let per_shader = model
            .drawables
            .iter()
            .map(|(_, d)| {
                d.shaders
                    .iter()
                    .map(|s| {
                        let opaque = rage_formats::shaders::alpha_is_not_opacity(s.name_hash);
                        Slots {
                            base: decode(d, s.base_texture(), opaque),
                            overlay: decode(d, s.overlay_texture(), false),
                            normal: decode(d, s.sampler(param::BUMP_TEX), false),
                            specular: decode(d, s.sampler(param::SPECULAR_TEX), false),
                        }
                    })
                    .collect()
            })
            .collect();
        Self { images, per_shader }
    }

    fn slot(&self, drawable: usize, shader: u16) -> Slots {
        self.per_shader.get(drawable).and_then(|s| s.get(shader as usize)).copied().unwrap_or_default()
    }

    fn image(&self, drawable: usize, shader: u16) -> Option<(&Image, &[u8])> {
        let image = &*self.images[self.slot(drawable, shader).base?];
        Some((image, image.rgba()?))
    }

    fn overlay_image(&self, drawable: usize, shader: u16) -> Option<(&Image, &[u8])> {
        let image = &*self.images[self.slot(drawable, shader).overlay?];
        Some((image, image.rgba()?))
    }

    /// The base texture of a drawable's shader, the overlay drawn over it with
    /// its alpha on the second UV set, the normal and the specular map.
    pub fn pictures(&self, drawable: usize, shader: u16) -> [Option<&Arc<Image>>; 4] {
        let Slots { base, overlay, normal, specular } = self.slot(drawable, shader);
        [base, overlay, normal, specular].map(|i| i.map(|i| &self.images[i]))
    }

    /// Lets go of what is on the graphics card now.
    pub fn release(&self) {
        self.images.iter().for_each(|i| i.release());
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
    // A little room around the model.
    let whole = radius / (fov / 2.0).sin() * 1.05;
    let distance = whole / view.zoom.max(0.05);
    let (cp, sp, cy, sy) = (view.pitch.cos(), view.pitch.sin(), view.yaw.cos(), view.yaw.sin());
    let towards = [cp * cy, cp * sy, sp];
    let forward = norm([-towards[0], -towards[1], -towards[2]]);
    let right = norm(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    // Nothing but the angles moves the camera: around the middle, on a circle.
    let eye = std::array::from_fn(|k| center[k] + distance * towards[k]);
    Camera { eye, forward, right, up, distance, radius, fov }
}

/// Renders the parts of `model` shown by `visible` into an RGBA image of
/// `width × height` with a transparent background.
pub fn render(
    model: &LoadedModel,
    look: &Look,
    materials: &Materials,
    view: View,
    width: usize,
    height: usize,
) -> Vec<u8> {
    // Bounds from the vertices (more reliable than stored bounds).
    match bounds(model, &look.parts) {
        Some((lo, hi)) => render_camera(model, look, materials, &orbit_camera(lo, hi, view), None, width, height),
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

/// Renders from any camera (see [`orbit_camera`], [`fly_camera`]); `skin`
/// poses skinned meshes (skinning matrices per skeleton bone).
pub fn render_camera(
    model: &LoadedModel,
    look: &Look,
    materials: &Materials,
    camera: &Camera,
    skin: Option<&[math::Mat4]>,
    width: usize,
    height: usize,
) -> Vec<u8> {
    render_scene(&[SceneItem { model, look, materials, transform: math::IDENTITY, skin }], camera, width, height)
}

/// One model of a scene: what is shown, where it stands and its pose.
pub struct SceneItem<'a> {
    pub model: &'a LoadedModel,
    pub look: &'a Look,
    pub materials: &'a Materials,
    /// Placement in the scene.
    pub transform: math::Mat4,
    pub skin: Option<&'a [math::Mat4]>,
}

/// A picture and its decoded pixels.
type Sampler<'a> = (&'a Image, &'a [u8]);

/// The image being drawn into.
struct Target {
    color: Vec<u8>,
    depth: Vec<f32>,
    w: usize,
    h: usize,
}

/// The pinhole camera of a render.
struct Projector {
    eye: V3,
    forward: V3,
    right: V3,
    up: V3,
    focal: f32,
    w: usize,
    h: usize,
}

impl Projector {
    /// Camera space → (screen x, screen y, view depth).
    #[inline]
    fn project(&self, p: V3) -> (f32, f32, f32) {
        let r = sub(p, self.eye);
        let z = dot(r, self.forward);
        (
            self.w as f32 / 2.0 + dot(r, self.right) * self.focal / z,
            self.h as f32 / 2.0 - dot(r, self.up) * self.focal / z,
            z,
        )
    }
}

/// How the vertices of one mesh get from the part's space into the scene.
struct Deform<'a> {
    part: &'a crate::models::Part,
    mesh: &'a rage_formats::drawable::Mesh,
    placement: &'a math::Mat4,
    skin: Option<&'a [math::Mat4]>,
}

impl Deform<'_> {
    /// Skinning acts in model space: after the part's placement. Parts
    /// attached to a bone follow that bone; others use the mesh weights.
    fn weights(&self, i: usize) -> Option<([u16; 4], [f32; 4])> {
        match self.part.bone {
            Some(b) => Some(([b as u16, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])),
            None => Some((*self.mesh.bone_indices.get(i)?, *self.mesh.bone_weights.get(i)?)),
        }
    }

    fn vertex(&self, i: usize, v: [f32; 3], direction: bool) -> [f32; 3] {
        let placed = if direction {
            math::transform_dir(&self.part.transform, v)
        } else {
            math::transform_point(&self.part.transform, v)
        };
        let posed = match (self.skin, self.weights(i)) {
            (Some(s), Some((idx, w))) => crate::anim::skin_point(s, idx, w, placed, direction),
            _ => placed,
        };
        if direction {
            math::transform_dir(self.placement, posed)
        } else {
            math::transform_point(self.placement, posed)
        }
    }
}

/// One mesh ready to be rasterised: its vertices in the scene and how to colour it.
struct MeshShade<'a> {
    mesh: &'a rage_formats::drawable::Mesh,
    tint: [f32; 4],
    img: Option<Sampler<'a>>,
    overlay: Option<Sampler<'a>>,
    has_uv: bool,
    has_n: bool,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    pts: Vec<(f32, f32, f32)>,
}

/// A triangle that passed culling: its corners, screen box and face normal.
struct Tri {
    idx: [usize; 3],
    pts: [(f32, f32, f32); 3],
    /// Reciprocal view depths of the corners.
    inv_z: [f32; 3],
    area: f32,
    face_n: V3,
    /// Pixel box: x0, x1, y0, y1 (inclusive).
    bounds: [usize; 4],
}

/// Interpolates a per-vertex 2D attribute with the perspective-correct weights.
#[inline]
fn lerp_uv(uvs: &[[f32; 2]], idx: [usize; 3], q: [f32; 3]) -> (f32, f32) {
    let [a, b, c] = idx;
    (q[0] * uvs[a][0] + q[1] * uvs[b][0] + q[2] * uvs[c][0], q[0] * uvs[a][1] + q[1] * uvs[b][1] + q[2] * uvs[c][1])
}

impl MeshShade<'_> {
    /// The texture colour times the tint, `None` for an alpha cut-out pixel.
    #[inline]
    fn base_color(&self, idx: [usize; 3], q: [f32; 3]) -> Option<[f32; 4]> {
        let mut base = [1.0f32; 4];
        if let (Some(img), true) = (self.img, self.has_uv) {
            let (u, v) = lerp_uv(&self.mesh.uvs, idx, q);
            let t = img.0.sample(img.1, u, v);
            if t[3] < 77 {
                return None; // alpha cut-out
            }
            base = [t[0] as f32 / 255.0, t[1] as f32 / 255.0, t[2] as f32 / 255.0, 1.0];
        }
        for (b, t) in base.iter_mut().zip(self.tint).take(3) {
            *b *= t;
        }
        Some(base)
    }

    #[inline]
    fn blend_overlay(&self, img: Sampler, idx: [usize; 3], q: [f32; 3], base: &mut [f32; 4]) {
        let (u, v) = lerp_uv(&self.mesh.uvs1, idx, q);
        let t = img.0.sample(img.1, u, v);
        let alpha = t[3] as f32 / 255.0;
        for k in 0..3 {
            base[k] += (t[k] as f32 / 255.0 - base[k]) * alpha;
        }
    }

    #[inline]
    fn normal_at(&self, tri: &Tri, q: [f32; 3]) -> V3 {
        if !self.has_n {
            return tri.face_n;
        }
        let [a, b, c] = tri.idx;
        let (na, nb, nc) = (self.normals[a], self.normals[b], self.normals[c]);
        norm([
            q[0] * na[0] + q[1] * nb[0] + q[2] * nc[0],
            q[0] * na[1] + q[1] * nb[1] + q[2] * nc[1],
            q[0] * na[2] + q[1] * nb[2] + q[2] * nc[2],
        ])
    }

    /// The pixel colour at perspective-correct weights `q`; `None` when it is cut out.
    #[inline]
    fn shade(&self, tri: &Tri, q: [f32; 3], forward: V3) -> Option<[u8; 3]> {
        let mut base = self.base_color(tri.idx, q)?;
        if let Some(img) = self.overlay {
            self.blend_overlay(img, tri.idx, q, &mut base);
        }
        let n = self.normal_at(tri, q);
        // Headlight plus a little sky light; double-sided.
        let light = 0.25 + 0.65 * dot(n, forward).abs() + 0.1 * n[2].max(0.0);
        Some([0, 1, 2].map(|k| (base[k] * light * 255.0).clamp(0.0, 255.0) as u8))
    }

    /// Culls and sets up triangle `tri` of the mesh; `None` when nothing of it is visible.
    #[inline]
    fn setup(&self, tri: &[u32], near: f32, size: (usize, usize)) -> Option<Tri> {
        let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let [a, b, c] = idx;
        let (pa, pb, pc) = (self.pts[a], self.pts[b], self.pts[c]);
        if pa.2 < near || pb.2 < near || pc.2 < near {
            return None;
        }
        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
        if area.abs() < 1e-9 {
            return None;
        }
        // Face normal for shading when vertex normals are missing.
        let face_n = norm(cross(sub(self.positions[b], self.positions[a]), sub(self.positions[c], self.positions[a])));
        let minx = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as usize;
        let maxx = pa.0.max(pb.0).max(pc.0).ceil().min(size.0 as f32 - 1.0);
        let miny = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as usize;
        let maxy = pa.1.max(pb.1).max(pc.1).ceil().min(size.1 as f32 - 1.0);
        if maxx < 0.0 || maxy < 0.0 {
            return None;
        }
        Some(Tri {
            idx,
            pts: [pa, pb, pc],
            inv_z: [1.0 / pa.2, 1.0 / pb.2, 1.0 / pc.2],
            area,
            face_n,
            bounds: [minx, maxx as usize, miny, maxy as usize],
        })
    }

    /// Rasterises one triangle into `target`.
    #[inline]
    fn draw_triangle(&self, tri: &Tri, target: &mut Target, forward: V3) {
        let [minx, maxx, miny, maxy] = tri.bounds;
        for y in miny..=maxy {
            for x in minx..=maxx {
                self.draw_pixel(tri, target, (x, y), forward);
            }
        }
    }

    #[inline]
    fn draw_pixel(&self, tri: &Tri, target: &mut Target, (x, y): (usize, usize), forward: V3) {
        let [pa, pb, pc] = tri.pts;
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let w0 = ((pb.0 - px) * (pc.1 - py) - (pb.1 - py) * (pc.0 - px)) / tri.area;
        let w1 = ((pc.0 - px) * (pa.1 - py) - (pc.1 - py) * (pa.0 - px)) / tri.area;
        let w2 = 1.0 - w0 - w1;
        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
            return;
        }
        let [iza, izb, izc] = tri.inv_z;
        let iz = w0 * iza + w1 * izb + w2 * izc;
        let z = 1.0 / iz;
        let di = y * target.w + x;
        if z >= target.depth[di] {
            return;
        }
        // Perspective-correct weights.
        let q = [w0 * iza * z, w1 * izb * z, w2 * izc * z];
        let Some(rgb) = self.shade(tri, q, forward) else { return };
        target.depth[di] = z;
        target.color[di * 4..di * 4 + 3].copy_from_slice(&rgb);
        target.color[di * 4 + 3] = 255;
    }
}

/// The mesh of `part` as it stands in the scene, `None` when it is hidden.
fn mesh_shade<'a>(
    item: &SceneItem<'a>,
    part: &crate::models::Part,
    mesh: &'a rage_formats::drawable::Mesh,
    cam: &Projector,
) -> Option<MeshShade<'a>> {
    let SceneItem { model, look, materials, transform: placement, skin } = *item;
    let tint = model.tint(look, part.drawable, mesh.shader)?;
    let overlay =
        materials.overlay_image(part.drawable, mesh.shader).filter(|_| mesh.uvs1.len() == mesh.positions.len());
    let deform = Deform { part, mesh, placement: &placement, skin };
    let positions: Vec<[f32; 3]> =
        mesh.positions.iter().enumerate().map(|(i, p)| deform.vertex(i, *p, false)).collect();
    let normals: Vec<[f32; 3]> = mesh.normals.iter().enumerate().map(|(i, n)| deform.vertex(i, *n, true)).collect();
    let pts = positions.iter().map(|p| cam.project(*p)).collect();
    Some(MeshShade {
        mesh,
        tint,
        img: materials.image(part.drawable, mesh.shader),
        overlay,
        has_uv: mesh.uvs.len() == mesh.positions.len(),
        has_n: mesh.normals.len() == mesh.positions.len(),
        positions,
        normals,
        pts,
    })
}

/// Renders several models into one RGBA image (software; for previews and
/// when there is no GPU).
pub fn render_scene(items: &[SceneItem], camera: &Camera, width: usize, height: usize) -> Vec<u8> {
    let (w, h) = (width.max(1), height.max(1));
    // Transparent background: the UI (or the PNG viewer) shows through.
    let mut target = Target { color: vec![0u8; w * h * 4], depth: vec![f32::INFINITY; w * h], w, h };
    let Camera { eye, forward, right, up, distance, fov, .. } = *camera;
    // Fit the field of view to the shorter side so tall previews do not crop.
    let focal = (w.min(h) as f32 / 2.0) / (fov / 2.0).tan();
    let near = distance * 0.01;
    let cam = Projector { eye, forward, right, up, focal, w, h };

    for item in items {
        for (part, d) in item.model.shown(&item.look.parts) {
            for mesh in &d.meshes {
                let Some(shade) = mesh_shade(item, part, mesh, &cam) else { continue };
                for tri in mesh.indices.as_chunks::<3>().0 {
                    if let Some(tri) = shade.setup(tri, near, (target.w, target.h)) {
                        shade.draw_triangle(&tri, &mut target, forward);
                    }
                }
            }
        }
    }
    target.color
}
