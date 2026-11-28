//! GPU renderer for model previews (wgpu: Vulkan, DirectX 12, Metal or GL).
//!
//! Renders offscreen with 4× multisampling and hands the image to Slint, so it
//! works whatever Slint itself draws with. Meshes are uploaded once per model
//! (with each part's placement baked in); a frame only updates the camera.
//! If no GPU is available, callers fall back to the software renderer.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use openrage_app::gta4_effects::{Image, Sprite};
use openrage_app::models::Look;
use openrage_app::render::{Camera, Image as Picture};
use rage_formats::drawable::param;
use rage_formats::math;
use rayon::prelude::*;
use wgpu::util::DeviceExt;

const SAMPLES: u32 = 4;
/// The most bones the skinning matrices cover.
const MAX_BONES: usize = 1024;
/// Bytes of a bone's matrix.
const BONE: u32 = 64;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADER: &str = r#"
struct Light {
    // xyz: position, w: range
    position: vec4<f32>,
    // rgb: colour × strength, a: 1 for a spot light
    color: vec4<f32>,
    // xyz: direction, w: cosine of half the cone angle
    direction: vec4<f32>,
};
struct Globals {
    view_proj: mat4x4<f32>,
    // xyz: viewing direction (headlight), w: number of lights
    forward: vec4<f32>,
    lights: array<Light, 16>,
    // Outdoors. xyz: where the camera is, w: how fast things fade into the
    // distance (per metre).
    eye: vec4<f32>,
    // The directions to the right and upper edge of the picture, w: there is a sky.
    right: vec4<f32>,
    up: vec4<f32>,
    // Towards the light, w: seconds, for what moves.
    sun: vec4<f32>,
    // Its colour, w: how much lamps shine.
    sun_colour: vec4<f32>,
    ambient: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    below: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
// Overlay drawn over the base colour with its alpha, on the second UV set
// (vehicle liveries and signs); transparent when there is none.
@group(1) @binding(2) var overlay: texture_2d<f32>;
// Flat and black when there is none.
@group(1) @binding(3) var normal_map: texture_2d<f32>;
@group(1) @binding(4) var specular_map: texture_2d<f32>;
// Skinning matrices per skeleton bone (identity when not animated).
@group(2) @binding(0) var<uniform> skin: array<mat4x4<f32>, {BONES}>;

// What a placement's colour says it is (its alpha).
const WATER: f32 = 2.0;
const LAMP: f32 = 3.0;

fn sky_colour(dir: vec3<f32>) -> vec3<f32> {
    var c = mix(globals.horizon.rgb, globals.zenith.rgb, pow(clamp(dir.z, 0.0, 1.0), 0.55));
    c = mix(c, globals.below.rgb, smoothstep(0.0, 0.25, -dir.z));
    let near = max(dot(dir, globals.sun.xyz), 0.0);
    return c + globals.sun_colour.rgb * (1.6 * pow(near, 1500.0) + 0.16 * pow(near, 12.0));
}

// The surface of water: waves that fade with the distance.
fn water_colour(world: vec3<f32>) -> vec3<f32> {
    let to_eye = globals.eye.xyz - world;
    let far = length(to_eye);
    let v = to_eye / max(far, 0.001);
    let t = globals.sun.w;
    var slope = vec2<f32>(0.0);
    var dir = vec2<f32>(0.8, 0.6);
    var long = 0.045;
    var high = 0.05;
    for (var i = 0; i < 6; i = i + 1) {
        let fade = exp(-far * long * 0.05);
        slope = slope + dir * cos(dot(world.xy, dir) * long * 6.2832 + t * (0.6 + f32(i) * 0.45)) * high * fade;
        dir = vec2<f32>(dir.x * 0.36 - dir.y * 0.93, dir.x * 0.93 + dir.y * 0.36);
        long = long * 1.9;
        high = high * 0.92;
    }
    let n = normalize(vec3<f32>(-slope, 1.0));
    let facing = clamp(dot(n, v), 0.0, 1.0);
    let mirror = 0.03 + 0.97 * pow(1.0 - facing, 5.0);
    var up = reflect(-v, n);
    up.z = abs(up.z);
    let day = globals.ambient.rgb + globals.sun_colour.rgb * 0.5;
    let deep = vec3<f32>(0.02, 0.16, 0.20) * day + vec3<f32>(0.02, 0.10, 0.09) * max(slope.x + slope.y, 0.0) * day;
    let spark = pow(max(dot(up, globals.sun.xyz), 0.0), 260.0);
    return mix(deep, sky_colour(normalize(up)), mirror) + globals.sun_colour.rgb * spark * 1.5;
}

struct SeaOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

// The sea: a square around the camera, up to the horizon.
@vertex
fn vs_sea(@builtin(vertex_index) i: u32) -> SeaOut {
    var o: SeaOut;
    let corner = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u)) * 2.0 - 1.0;
    o.world = vec3<f32>(globals.eye.xy + corner * 40000.0, 0.0);
    o.clip = globals.view_proj * vec4<f32>(o.world, 1.0);
    return o;
}

@fragment
fn fs_sea(i: SeaOut) -> @location(0) vec4<f32> {
    let ray = i.world - globals.eye.xyz;
    let far = length(ray);
    let dir = ray / max(far, 0.001);
    let haze = sky_colour(normalize(vec3<f32>(dir.xy, 0.0)));
    let lit = mix(haze, water_colour(i.world), exp(-far * globals.eye.w)) * globals.ambient.w;
    // Looking down, the ground shows through a little.
    let alpha = mix(0.97, 0.82, clamp(-dir.z, 0.0, 1.0));
    return vec4<f32>(lit * alpha, alpha);
}

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) dir: vec3<f32>,
};

// A triangle over the whole picture, behind everything.
@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    var o: SkyOut;
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    o.clip = vec4<f32>(x, y, 0.0, 1.0);
    o.dir = globals.forward.xyz + globals.right.xyz * x + globals.up.xyz * y;
    return o;
}

@fragment
fn fs_sky(i: SkyOut) -> @location(0) vec4<f32> {
    return vec4<f32>(sky_colour(normalize(i.dir)) * globals.ambient.w, 1.0);
}

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) uv1: vec2<f32>,
    @location(11) shading: vec4<f32>,
    // Per placement: where it is, and the material's colour (multiplied with
    // the texture).
    @location(6) model_0: vec4<f32>,
    @location(7) model_1: vec4<f32>,
    @location(8) model_2: vec4<f32>,
    @location(9) model_3: vec4<f32>,
    @location(10) tint: vec4<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) world: vec3<f32>,
    @location(4) tint: vec3<f32>,
    @location(5) @interpolate(flat) kind: f32,
    // How much sky it sees, and how much of what is put on the base shows.
    @location(6) shading: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    var pos = vec4<f32>(v.pos, 1.0);
    var normal = v.normal.xyz;
    let total = v.weights.x + v.weights.y + v.weights.z + v.weights.w;
    if (total > 0.0) {
        let m = (skin[min(v.joints.x, {LAST_BONE})] * v.weights.x + skin[min(v.joints.y, {LAST_BONE})] * v.weights.y
            + skin[min(v.joints.z, {LAST_BONE})] * v.weights.z + skin[min(v.joints.w, {LAST_BONE})] * v.weights.w) * (1.0 / total);
        pos = m * pos;
        normal = (m * vec4<f32>(normal, 0.0)).xyz;
    }
    let model = mat4x4<f32>(v.model_0, v.model_1, v.model_2, v.model_3);
    let world = model * pos;
    o.clip = globals.view_proj * world;
    o.world = world.xyz;
    o.normal = (model * vec4<f32>(normal, 0.0)).xyz;
    o.tint = v.tint.rgb;
    o.kind = v.tint.a;
    o.shading = v.shading.ra;
    o.uv = v.uv;
    o.uv1 = v.uv1;
    return o;
}

// The colour, with the base texture's alpha.
fn shade(i: VsOut, base: vec4<f32>) -> vec4<f32> {
    var n = i.normal;
    if (dot(n, n) < 1e-6) {
        n = vec3<f32>(0.0, 0.0, 1.0);
    }
    n = normalize(n);
    // The normal map bends it, along the directions the texture runs in.
    let bump = textureSample(normal_map, samp, i.uv).xy * 2.0 - vec2<f32>(1.0);
    let px = dpdx(i.world);
    let py = dpdy(i.world);
    let ux = dpdx(i.uv);
    let uy = dpdy(i.uv);
    let shine = textureSample(specular_map, samp, i.uv);
    let layer = textureSample(overlay, samp, i.uv1);
    let det = ux.x * uy.y - uy.x * ux.y;
    if (abs(det) > 1e-12 && dot(bump, bump) > 0.0004) {
        let t = (px * uy.y - py * ux.y) / det;
        let b = (py * ux.x - px * uy.x) / det;
        let tangent = t - n * dot(n, t);
        let bitangent = b - n * dot(n, b);
        if (dot(tangent, tangent) > 1e-12 && dot(bitangent, bitangent) > 1e-12) {
            let z = sqrt(max(1.0 - dot(bump, bump), 0.05));
            n = normalize(n * z + normalize(tangent) * bump.x + normalize(bitangent) * bump.y);
        }
    }
    // Headlight plus a little sky light; double-sided.
    var light = vec3<f32>(0.25 + 0.65 * abs(dot(n, globals.forward.xyz)) + 0.1 * max(n.z, 0.0));
    if (globals.right.w > 0.5) {
        // Outdoors: the sun, and the sky from above.
        let to_eye = globals.eye.xyz - i.world;
        let m = select(-n, n, dot(n, to_eye) >= 0.0);
        light = globals.sun_colour.rgb * max(dot(m, globals.sun.xyz), 0.0)
            + globals.ambient.rgb * (0.85 + 0.35 * m.z);
        light = light * mix(0.35, 1.0, i.shading.x);
        if (abs(i.kind - LAMP) < 0.5) {
            light = max(light, vec3<f32>(globals.sun_colour.w));
        }
    }
    let count = u32(globals.forward.w);
    if (count > 0u) {
        // With scene lights the headlight is dimmer and the lights add up.
        light = vec3<f32>(0.22 + 0.3 * abs(dot(n, globals.forward.xyz)) + 0.05 * max(n.z, 0.0));
        for (var k = 0u; k < min(count, 16u); k = k + 1u) {
            let l = globals.lights[k];
            let to = l.position.xyz - i.world;
            let d = length(to);
            if (d >= l.position.w || d < 1e-4) {
                continue;
            }
            let dir = to / d;
            let fall = 1.0 - d / l.position.w;
            var amount = fall * fall * abs(dot(n, dir));
            if (l.color.a > 0.5) {
                let c = dot(-dir, normalize(l.direction.xyz));
                amount = amount * smoothstep(l.direction.w, mix(l.direction.w, 1.0, 0.3), c);
            }
            light = light + l.color.rgb * amount;
        }
    }
    let colour = mix(base.rgb * i.tint, layer.rgb, layer.a * i.shading.y);
    // What shines towards the viewer.
    let glint = pow(abs(dot(n, globals.forward.xyz)), 24.0) * max(max(shine.r, shine.g), shine.b) * 0.25;
    var lit = colour * light + vec3<f32>(glint);
    if (globals.right.w > 0.5) {
        lit = colour * light;
        if (abs(i.kind - WATER) < 0.5) {
            lit = water_colour(i.world);
        }
        let ray = i.world - globals.eye.xyz;
        let far = length(ray);
        let dir = ray / max(far, 0.001);
        let haze = sky_colour(normalize(vec3<f32>(dir.xy, max(dir.z, 0.0))));
        lit = mix(haze, lit, exp(-far * globals.eye.w)) * globals.ambient.w;
    }
    return vec4<f32>(lit, base.a);
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(tex, samp, i.uv);
    let c = shade(i, base);
    if (base.a < 0.3) {
        discard;
    }
    return vec4<f32>(c.rgb, 1.0);
}

// What lies on other things (road markings, dirt): blended over them.
@fragment
fn fs_decal(i: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(tex, samp, i.uv);
    let c = shade(i, base);
    let alpha = base.a * i.shading.y;
    return vec4<f32>(c.rgb * alpha, alpha);
}

// Particles: camera-facing squares, built on the CPU in world space.
struct SpriteIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) colour: vec4<f32>,
};
struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

@vertex
fn vs_sprite(v: SpriteIn) -> SpriteOut {
    var o: SpriteOut;
    o.clip = globals.view_proj * vec4<f32>(v.pos, 1.0);
    o.uv = v.uv;
    o.colour = v.colour;
    return o;
}

// Premultiplied: blended over (alpha) or added (glows) by the pipeline.
@fragment
fn fs_sprite(i: SpriteOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.colour;
    return vec4<f32>(c.rgb * c.a, c.a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    /// In 127ths.
    normal: [i8; 4],
    uv: [f32; 2],
    /// In 255ths.
    weights: [u8; 4],
    joints: [u16; 4],
    uv1: [f32; 2],
    /// [`rage_formats::drawable::Mesh::shading`], in 255ths.
    shading: [u8; 4],
}

fn small_normal(n: [f32; 3]) -> [i8; 4] {
    let len = n.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    let [x, y, z] = n.map(|v| (v / len * 127.0).round() as i8);
    [x, y, z, 0]
}

fn small_weights(w: [f32; 4]) -> [u8; 4] {
    w.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    colour: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuLight {
    position: [f32; 4],
    color: [f32; 4],
    direction: [f32; 4],
}

/// The most lights a scene uses at once.
pub const MAX_LIGHTS: usize = 16;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [f32; 16],
    forward: [f32; 4],
    lights: [GpuLight; MAX_LIGHTS],
    eye: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    sun: [f32; 4],
    sun_colour: [f32; 4],
    ambient: [f32; 4],
    zenith: [f32; 4],
    horizon: [f32; 4],
    below: [f32; 4],
}

/// A light of a scene: position, colour × strength, reach, and for spot
/// lights the direction and full cone angle.
#[derive(Debug, Clone, Copy)]
pub struct SceneLight {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub range: f32,
    pub spot: Option<([f32; 3], f32)>,
}

/// A placement of a draw.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    model: math::Mat4,
    tint: [f32; 4],
}

/// How a scene outdoors looks.
#[derive(Debug, Clone, Copy)]
pub struct Outdoors {
    /// How fast things fade into the distance, per metre.
    pub haze: f32,
    /// Hour of the day.
    pub hour: f32,
    pub brightness: f32,
    /// For what moves.
    pub seconds: f32,
    /// There is a sea at height 0.
    pub sea: bool,
}

/// Marks a material's colour as water, or as a lamp that shines at night.
pub const WATER: f32 = 2.0;
pub const LAMP: f32 = 3.0;

/// The light at an hour.
struct Daylight {
    /// Towards the sun or the moon.
    towards: [f32; 3],
    colour: [f32; 3],
    /// How much lamps shine.
    lamps: f32,
    /// From all sides.
    ambient: [f32; 3],
    zenith: [f32; 3],
    horizon: [f32; 3],
    /// Below the horizon.
    below: [f32; 3],
}

fn light_at(hour: f32) -> Daylight {
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
    let step = |a: f32, b: f32, v: f32| {
        let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let angle = (hour - 6.0) / 12.0 * std::f32::consts::PI;
    // Up in the east, over the south, down in the west.
    let sun = [angle.cos() * 0.92, -0.38, angle.sin()];
    let height = angle.sin();
    let day = step(-0.3, 0.25, height);
    let dusk = (-(height / 0.2).powi(2)).exp();
    let colour: [f32; 3] = if height > -0.06 {
        mix([0.75, 0.34, 0.13], [0.66, 0.62, 0.55], step(0.0, 0.4, height)).map(|c: f32| c * step(-0.06, 0.1, height))
    } else {
        [0.16, 0.20, 0.30]
    };
    let light = if height > -0.06 { sun } else { [-sun[0], sun[1], -sun[2]] };
    let len = light.iter().map(|v| v * v).sum::<f32>().sqrt();
    let horizon = mix([0.09, 0.11, 0.18], [0.74, 0.82, 0.89], day);
    let lamps = 1.0 - step(-0.02, 0.25, height);
    Daylight {
        towards: light.map(|v| v / len),
        colour,
        lamps: lamps * 1.6,
        ambient: mix([0.15, 0.18, 0.26], [0.36, 0.40, 0.46], day),
        zenith: mix([0.02, 0.03, 0.08], [0.22, 0.45, 0.78], day),
        horizon: mix(horizon, [0.95, 0.52, 0.28], dusk * 0.75),
        below: mix([0.04, 0.05, 0.08], [0.40, 0.48, 0.55], day),
    }
}

/// Outdoors, models smaller than this in the picture are left out (pixels).
const SMALLEST: f32 = 1.6;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// Particles blended over the scene, and particles that add light.
    sprite_blend: wgpu::RenderPipeline,
    sprite_add: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
    sea: wgpu::RenderPipeline,
    /// Block-compressed textures can be used as they are.
    blocks: bool,
    globals_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    skin_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// White 1×1 texture for untextured meshes (their colour is the tint).
    white: wgpu::TextureView,
    /// The same in grey.
    grey: wgpu::TextureView,
    /// Transparent 1×1 texture for meshes without an overlay.
    clear: wgpu::TextureView,
    /// 1×1 normal map for meshes without one.
    flat: wgpu::TextureView,
    /// Bones the skinning can use: devices differ.
    bones: usize,
}

struct Draw {
    part: usize,
    drawable: usize,
    shader: u16,
    /// In the model's buffers.
    base_vertex: i32,
    indices: std::ops::Range<u32>,
    texture: usize,
    /// Lies on other things.
    decal: bool,
    /// Bounding sphere (centre, radius) in the model's space, for culling.
    sphere: ([f32; 3], f32),
}

/// A model uploaded to the GPU.
struct GpuModel {
    owner: Weak<crate::ModelPreview>,
    draws: Vec<Draw>,
    /// Models with the same number are in the same buffers.
    batch: u64,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Bind groups: 0 is the plain texture, then one per combination of
    /// decoded images.
    textures: Vec<wgpu::BindGroup>,
}

/// Render targets reused while the size stays the same.
struct Targets {
    size: (u32, u32),
    msaa: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    readback: [wgpu::Buffer; 2],
    /// A picture on its way back: for whom, its buffer and when it is done.
    pending: Option<(u64, usize, wgpu::SubmissionIndex)>,
    padded_row: u32,
}

#[derive(Default)]
struct State {
    models: Vec<GpuModel>,
    targets: Option<Targets>,
    /// Bone palette of items without a skin (all identity), shared by them.
    rest_pose: Option<wgpu::BindGroup>,
    /// Particle textures, uploaded once each.
    sprite_textures: Vec<(Weak<Image>, wgpu::BindGroup)>,
}

/// The graphics device; `None` if there is no usable one.
static GPU: LazyLock<Option<Gpu>> = LazyLock::new(init);

/// Models uploaded by other threads.
static READY: Mutex<Vec<GpuModel>> = Mutex::new(Vec::new());

/// The textures of pictures that several models share.
type Views = HashMap<usize, (Weak<Picture>, wgpu::TextureView)>;
static VIEWS: LazyLock<Mutex<Views>> = LazyLock::new(Default::default);

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn init() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .ok()?;
    let mut limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
    // People of GTA VI have about 600 bones.
    let wanted = adapter.limits().max_uniform_buffer_binding_size.min((MAX_BONES as u32 * BONE).into());
    limits.max_uniform_buffer_binding_size = limits.max_uniform_buffer_binding_size.max(wanted);
    let bones = (limits.max_uniform_buffer_binding_size / u64::from(BONE)) as usize;
    let bones = bones.min(MAX_BONES);
    let blocks = adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("openrage"),
        required_features: if blocks { wgpu::Features::TEXTURE_COMPRESSION_BC } else { wgpu::Features::empty() },
        required_limits: limits,
        ..Default::default()
    }))
    .ok()?;

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("model"),
        source: wgpu::ShaderSource::Wgsl(shader_source(bones).into()),
    });
    let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("globals"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("texture"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let skin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("skin"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("model"),
        bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout), Some(&skin_layout)],
        immediate_size: 0,
    });
    let model_pipeline = |decal: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint16x4, 5 => Float32x2, 11 => Unorm8x4],
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4],
                }),
            ],
        },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(!decal),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            // Depth is larger nearer to the camera.
            bias: if decal {
                wgpu::DepthBiasState { constant: 24, slope_scale: 1.5, clamp: 0.0 }
            } else {
                Default::default()
            },
        }),
        multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(if decal { "fs_decal" } else { "fs_main" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: decal.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
    };
    let (pipeline, decal) = (model_pipeline(false), model_pipeline(true));
    let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky"),
        bind_group_layouts: &[Some(&globals_layout)],
        immediate_size: 0,
    });
    let outdoor_pipeline = |vertex, fragment, topology, blend| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(vertex),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(vertex),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState { topology, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let sky = outdoor_pipeline("vs_sky", "fs_sky", wgpu::PrimitiveTopology::TriangleList, None);
    let over = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
    let sea = outdoor_pipeline("vs_sea", "fs_sea", wgpu::PrimitiveTopology::TriangleStrip, Some(over));
    let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sprite"),
        bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)],
        immediate_size: 0,
    });
    let sprite_pipeline = |blend: wgpu::BlendState| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&sprite_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SpriteVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            // Hidden behind models, but not hiding each other.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_sprite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let sprite_blend = sprite_pipeline(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
    let sprite_add = sprite_pipeline(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    let white = upload_texture(&device, &queue, 1, 1, &[255, 255, 255, 255]);
    let grey = upload_texture(&device, &queue, 1, 1, &[118, 118, 116, 255]);
    let clear = upload_texture(&device, &queue, 1, 1, &[0, 0, 0, 0]);
    let flat = upload_texture(&device, &queue, 1, 1, &[128, 128, 255, 255]);
    Some(Gpu {
        sprite_blend,
        sprite_add,
        sky,
        decal,
        sea,
        blocks,
        device,
        queue,
        pipeline,
        globals_layout,
        texture_layout,
        skin_layout,
        sampler,
        white,
        grey,
        clear,
        flat,
        bones,
    })
}

/// Half of a picture in each direction.
fn halved(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity((hw * hh * 4) as usize);
    for y in 0..hh {
        let rows = [(y * 2).min(h - 1), (y * 2 + 1).min(h - 1)];
        for x in 0..hw {
            let columns = [(x * 2).min(w - 1), (x * 2 + 1).min(w - 1)];
            let at = |x: u32, y: u32| ((y * w + x) * 4) as usize;
            let from =
                [at(columns[0], rows[0]), at(columns[1], rows[0]), at(columns[0], rows[1]), at(columns[1], rows[1])];
            for c in 0..4 {
                let sum: u32 = from.iter().map(|i| u32::from(rgba[i + c])).sum();
                out.push(((sum + 2) / 4) as u8);
            }
        }
    }
    (hw, hh, out)
}

/// Uploads RGBA pixels as a texture, with smaller copies for the distance.
fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
    let levels = w.max(h).max(1).ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Colors are passed through unchanged, like the software renderer.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let write = |level: u32, w: u32, h: u32, rgba: &[u8]| {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    };
    write(0, w, h, rgba);
    let mut smaller: Option<(u32, u32, Vec<u8>)> = None;
    for level in 1..levels {
        let next = match &smaller {
            None => halved(w, h, rgba),
            Some((w, h, rgba)) => halved(*w, *h, rgba),
        };
        write(level, next.0, next.1, &next.2);
        smaller = Some(next);
    }
    texture.create_view(&Default::default())
}

/// The texture of a picture on a model, made once for all that have it.
fn picture_view(gpu: &Gpu, picture: &Arc<Picture>) -> Option<wgpu::TextureView> {
    let key = Arc::as_ptr(picture) as usize;
    {
        let views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((of, view)) = views.get(&key)
            && of.upgrade().is_some_and(|of| Arc::ptr_eq(&of, picture))
        {
            return Some(view.clone());
        }
    }
    let (w, h) = picture.size();
    let as_it_is = picture.source().filter(|_| gpu.blocks).and_then(|t| upload_blocks(gpu, &t));
    let view = match as_it_is {
        Some(view) => view,
        None => upload_texture(&gpu.device, &gpu.queue, w as u32, h as u32, picture.rgba()?),
    };
    let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    if views.len().is_power_of_two() {
        views.retain(|_, (of, _)| of.strong_count() > 0);
    }
    views.insert(key, (Arc::downgrade(picture), view.clone()));
    Some(view)
}

/// Uploads a block-compressed texture; `None` if it is of another kind.
fn upload_blocks(gpu: &Gpu, t: &rage_formats::texture::Texture) -> Option<wgpu::TextureView> {
    // Colours are passed through unchanged, so none is declared sRGB.
    let format = match t.format.0 {
        70..=72 => wgpu::TextureFormat::Bc1RgbaUnorm,
        76..=78 => wgpu::TextureFormat::Bc3RgbaUnorm,
        82 | 83 => wgpu::TextureFormat::Bc5RgUnorm,
        97..=99 => wgpu::TextureFormat::Bc7RgbaUnorm,
        _ => return None,
    };
    if t.depth > 1 || !t.width.is_multiple_of(4) || !t.height.is_multiple_of(4) || t.levels == 0 {
        return None;
    }
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        mip_level_count: t.levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut at = 0;
    for level in 0..t.levels {
        let (w, h) = ((t.width >> level).max(1), (t.height >> level).max(1));
        let (row, size) = t.format.pitch(w, h)?;
        let data = t.data.get(at..at + size)?;
        at += size;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: None },
            wgpu::Extent3d { width: w.next_multiple_of(4), height: h.next_multiple_of(4), depth_or_array_layers: 1 },
        );
    }
    Some(texture.create_view(&Default::default()))
}

/// The shader, for devices that can skin with this many bones.
fn shader_source(bones: usize) -> String {
    SHADER.replace("{BONES}", &bones.to_string()).replace("{LAST_BONE}", &format!("{}u", bones - 1))
}

/// A texture that is only a colour.
fn plain_bind_group(gpu: &Gpu, base: &wgpu::TextureView) -> wgpu::BindGroup {
    texture_bind_group(gpu, base, &gpu.clear, &gpu.flat, &gpu.clear)
}

/// A material's textures: base colour, overlay, normal and specular map.
fn texture_bind_group(
    gpu: &Gpu,
    base: &wgpu::TextureView,
    overlay: &wgpu::TextureView,
    normal: &wgpu::TextureView,
    specular: &wgpu::TextureView,
) -> wgpu::BindGroup {
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &gpu.texture_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(base) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(overlay) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(normal) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(specular) },
        ],
    })
}

/// A model on its way to the graphics card.
struct Built {
    owner: Weak<crate::ModelPreview>,
    draws: Vec<Draw>,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    textures: Vec<wgpu::BindGroup>,
}

/// Every part of the model (placements baked into the vertices), with its
/// textures uploaded.
fn build(gpu: &Gpu, preview: &Arc<crate::ModelPreview>) -> Built {
    let model = &preview.loaded;
    let mut asm = Assembly {
        gpu,
        model,
        materials: &preview.materials,
        vertices: Vec::new(),
        indices: Vec::new(),
        textures: vec![plain_bind_group(gpu, &gpu.white)],
        // Pictures → bind group.
        texture_of: HashMap::new(),
        draws: Vec::new(),
    };
    for (part_index, part) in model.parts.iter().enumerate() {
        let Some((_, d)) = model.drawables.get(part.drawable) else { continue };
        for mesh in &d.meshes {
            asm.add_mesh(part_index, part, d, mesh);
        }
    }
    Built {
        owner: Arc::downgrade(preview),
        draws: asm.draws,
        vertices: asm.vertices,
        indices: asm.indices,
        textures: asm.textures,
    }
}

/// What [`build`] collects while it walks the model's meshes.
struct Assembly<'a> {
    gpu: &'a Gpu,
    model: &'a openrage_app::models::LoadedModel,
    materials: &'a openrage_app::render::Materials,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    textures: Vec<wgpu::BindGroup>,
    texture_of: HashMap<[usize; 4], usize>,
    draws: Vec<Draw>,
}

impl Assembly<'_> {
    /// Adds one mesh of `part` to the buffers and the draw list (skipping decals on plain materials).
    fn add_mesh(
        &mut self,
        part_index: usize,
        part: &openrage_app::models::Part,
        d: &rage_formats::drawable::Drawable,
        mesh: &rage_formats::drawable::Mesh,
    ) {
        if mesh.positions.is_empty() || mesh.indices.is_empty() {
            return;
        }
        let has_uv = mesh.uvs.len() == mesh.positions.len();
        let has_uv1 = mesh.uvs1.len() == mesh.positions.len();
        // The second layer runs like the first.
        let layered = d
            .shaders
            .get(usize::from(mesh.shader))
            .is_some_and(|s| s.sampler(param::LAYER_1_TEX).is_some() && s.sampler(param::DIFFUSE_TEX2).is_none());
        let vertices = placed_vertices(part, mesh, layered);
        let usable = [has_uv, has_uv1 || (layered && has_uv), has_uv, has_uv];
        let mut pictures = self.materials.pictures(part.drawable, mesh.shader);
        pictures.iter_mut().zip(usable).for_each(|(p, usable)| *p = p.filter(|_| usable));
        // A shader without a picture among others of its material that have
        // one, whose colour says nothing about it.
        let plain = pictures[0].is_none()
            && self
                .model
                .material_index(part.drawable, mesh.shader)
                .and_then(|m| self.model.materials.get(m))
                .is_some_and(|m| m.textured);
        let decal = d
            .shaders
            .get(usize::from(mesh.shader))
            .is_some_and(|s| rage_formats::names::value(s.name_hash).contains("decal"));
        if plain && decal {
            return;
        }
        let mut key = pictures.map(|p| p.map_or(0, |p| Arc::as_ptr(p) as usize));
        key[0] = if plain { 1 } else { key[0] };
        let (gpu, textures) = (self.gpu, &mut self.textures);
        let texture = *self.texture_of.entry(key).or_insert_with(|| push_texture_group(gpu, textures, pictures, plain));
        self.draws.push(Draw {
            sphere: bounding_sphere(&vertices),
            part: part_index,
            drawable: part.drawable,
            shader: mesh.shader,
            base_vertex: self.vertices.len() as i32,
            indices: self.indices.len() as u32..(self.indices.len() + mesh.indices.len()) as u32,
            texture,
            decal,
        });
        self.vertices.extend_from_slice(&vertices);
        self.indices.extend_from_slice(&mesh.indices);
    }
}

/// Parts attached to a bone follow it; others use the mesh's weights.
fn skin_weights(bone: Option<u16>, mesh: &rage_formats::drawable::Mesh, i: usize) -> [u8; 4] {
    match bone {
        Some(_) => [255, 0, 0, 0],
        None if mesh.bone_weights.len() == mesh.positions.len() => small_weights(mesh.bone_weights[i]),
        None => [0; 4],
    }
}

/// The bones of vertex `i`: the part's own bone, or the mesh's skinning.
fn skin_joints(bone: Option<u16>, mesh: &rage_formats::drawable::Mesh, i: usize) -> [u16; 4] {
    match bone {
        Some(b) => [b, 0, 0, 0],
        None if mesh.bone_weights.len() == mesh.positions.len() => mesh.bone_indices[i],
        None => [0; 4],
    }
}

/// The mesh's vertices with the part's placement baked in.
fn placed_vertices(
    part: &openrage_app::models::Part,
    mesh: &rage_formats::drawable::Mesh,
    layered: bool,
) -> Vec<Vertex> {
    let has_n = mesh.normals.len() == mesh.positions.len();
    let has_uv = mesh.uvs.len() == mesh.positions.len();
    let has_uv1 = mesh.uvs1.len() == mesh.positions.len();
    let bone = part.bone.map(|b| b as u16);
    (0..mesh.positions.len())
        .map(|i| Vertex {
            pos: math::transform_point(&part.transform, mesh.positions[i]),
            normal: if has_n { small_normal(math::transform_dir(&part.transform, mesh.normals[i])) } else { [0; 4] },
            uv: if has_uv { mesh.uvs[i] } else { [0.0; 2] },
            weights: skin_weights(bone, mesh, i),
            joints: skin_joints(bone, mesh, i),
            uv1: match (layered, has_uv1) {
                (true, _) if has_uv => mesh.uvs[i],
                (false, true) => mesh.uvs1[i],
                _ => [0.0; 2],
            },
            shading: mesh.shading.get(i).copied().unwrap_or([255; 4]),
        })
        .collect()
}

/// Uploads the pictures of one material as a bind group and returns its index in `textures`.
fn push_texture_group(
    gpu: &Gpu,
    textures: &mut Vec<wgpu::BindGroup>,
    pictures: [Option<&Arc<Picture>>; 4],
    plain: bool,
) -> usize {
    let [base, overlay, normal, specular] = pictures.map(|p| p.and_then(|p| picture_view(gpu, p)));
    textures.push(texture_bind_group(
        gpu,
        base.as_ref().unwrap_or(if plain { &gpu.grey } else { &gpu.white }),
        overlay.as_ref().unwrap_or(&gpu.clear),
        normal.as_ref().unwrap_or(&gpu.flat),
        specular.as_ref().unwrap_or(&gpu.clear),
    ));
    textures.len() - 1
}

/// Centre and radius of the box around the vertices.
fn bounding_sphere(vertices: &[Vertex]) -> ([f32; 3], f32) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v.pos[k]);
            hi[k] = hi[k].max(v.pos[k]);
        }
    }
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let radius = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
    (centre, radius)
}

/// Bytes of vertices or indices that share a buffer.
const BATCH: usize = 64 << 20;

/// Puts models into buffers they share: changing buffers takes longer than
/// drawing a small model.
fn upload(gpu: &Gpu, built: Vec<Built>) -> Vec<GpuModel> {
    static NUMBER: AtomicU64 = AtomicU64::new(0);
    let mut models = Vec::with_capacity(built.len());
    let mut rest = built.into_iter().peekable();
    while rest.peek().is_some() {
        let (mut vertices, mut indices): (Vec<Vertex>, Vec<u32>) = (Vec::new(), Vec::new());
        let mut batch = Vec::new();
        while let Some(next) = rest.next_if(|b| {
            let bytes = |v: usize, i: usize| (v * std::mem::size_of::<Vertex>()).max(i * 4);
            batch.is_empty() || bytes(vertices.len() + b.vertices.len(), indices.len() + b.indices.len()) <= BATCH
        }) {
            let Built { owner, mut draws, vertices: v, indices: i, textures } = next;
            for d in &mut draws {
                d.base_vertex += vertices.len() as i32;
                d.indices = d.indices.start + indices.len() as u32..d.indices.end + indices.len() as u32;
            }
            vertices.extend_from_slice(&v);
            indices.extend_from_slice(&i);
            batch.push((owner, draws, textures));
        }
        // An empty buffer cannot be used.
        if indices.is_empty() {
            vertices.push(bytemuck::Zeroable::zeroed());
            indices.push(0);
        }
        let buffer = |contents: &[u8], usage| {
            gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents, usage })
        };
        let vertices = buffer(bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX);
        let indices = buffer(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX);
        let number = NUMBER.fetch_add(1, Ordering::Relaxed);
        models.extend(batch.into_iter().map(|(owner, draws, textures)| GpuModel {
            owner,
            draws,
            batch: number,
            vertices: vertices.clone(),
            indices: indices.clone(),
            textures,
        }));
    }
    models
}

/// Uploads models ahead of their first picture; from any thread.
pub fn prepare(previews: &[Arc<crate::ModelPreview>]) {
    if let Some(gpu) = &*GPU {
        let built = previews.par_iter().map(|p| build(gpu, p)).collect();
        previews.iter().for_each(|p| p.materials.release());
        let models = upload(gpu, built);
        READY.lock().unwrap_or_else(|e| e.into_inner()).extend(models);
    }
}

fn targets(gpu: &Gpu, w: u32, h: u32) -> Targets {
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let make = |format, samples, usage| {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let msaa = make(COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let resolve = make(COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth = make(DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let padded_row = (4 * w).div_ceil(256) * 256;
    let readback = || {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    };
    Targets {
        size: (w, h),
        msaa: msaa.create_view(&Default::default()),
        resolve_view: resolve.create_view(&Default::default()),
        resolve,
        depth: depth.create_view(&Default::default()),
        readback: [readback(), readback()],
        pending: None,
        padded_row,
    }
}

/// Column-major view-projection matrix for a camera (Z-up world).
/// The six planes (a, b, c, d; inside where a·x + b·y + c·z + d ≥ 0) of a
/// view-projection matrix with depth 0..1.
fn frustum_planes(m: &[f32; 16]) -> [[f32; 4]; 6] {
    let row = |i: usize| [m[i], m[4 + i], m[8 + i], m[12 + i]];
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let add = |a: [f32; 4], b: [f32; 4], s: f32| std::array::from_fn(|k| a[k] + s * b[k]);
    let mut planes = [add(r3, r0, 1.0), add(r3, r0, -1.0), add(r3, r1, 1.0), add(r3, r1, -1.0), r2, add(r3, r2, -1.0)];
    for p in &mut planes {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-12);
        *p = p.map(|v| v / len);
    }
    planes
}

/// The largest factor a placement scales by.
fn scale_of(model: &math::Mat4) -> f32 {
    (0..3)
        .map(|k| (model[k * 4].powi(2) + model[k * 4 + 1].powi(2) + model[k * 4 + 2].powi(2)).sqrt())
        .fold(0.0f32, f32::max)
}

/// Whether a bounding sphere in model space, placed by `model`, touches the view.
fn sphere_visible(planes: &[[f32; 4]; 6], model: &math::Mat4, (centre, radius): ([f32; 3], f32)) -> bool {
    let c = math::transform_point(model, centre);
    let r = radius * scale_of(model);
    planes.iter().all(|p| p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] >= -r)
}

fn view_projection(cam: &Camera, w: u32, h: u32, near: f32, far: f32) -> [f32; 16] {
    let (f, r, u, e) = (cam.forward, cam.right, cam.up, cam.eye);
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // View: rows are right, up, -forward (right-handed, looking down -Z).
    let view = [
        r[0],
        u[0],
        -f[0],
        0.0,
        r[1],
        u[1],
        -f[1],
        0.0,
        r[2],
        u[2],
        -f[2],
        0.0,
        -dot(r, e),
        -dot(u, e),
        dot(f, e),
        1.0,
    ];
    // Fit the field of view to the shorter side (like the software renderer).
    let short = w.min(h) as f32;
    let t = (cam.fov / 2.0).tan();
    let (sx, sy) = (short / w as f32 / t, short / h as f32 / t);
    // Perspective with depth 1 (near) to 0: floats are finest at 0, which
    // makes up for the far being so dense. Maps are kilometres wide.
    let a = near / (far - near);
    let proj = [sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, a, -1.0, 0.0, 0.0, a * far, 0.0];
    math::mul(&proj, &view)
}

/// Renders the parts of `model` shown by `visible` from `camera`. Returns a
/// premultiplied RGBA image, or `None` without a usable GPU.
pub fn render(
    preview: &Arc<crate::ModelPreview>,
    look: &Look,
    camera: &Camera,
    skin: Option<&[math::Mat4]>,
    w: u32,
    h: u32,
) -> Option<slint::Image> {
    render_scene(&[SceneItem { preview, look, model: math::IDENTITY, skin }], &Around::default(), camera, w, h)
}

/// One model of a scene: what is shown, where it stands and its pose.
pub struct SceneItem<'a> {
    pub preview: &'a Arc<crate::ModelPreview>,
    pub look: &'a Look,
    /// Placement in the scene.
    pub model: math::Mat4,
    pub skin: Option<&'a [math::Mat4]>,
}

/// A run of sprites drawn alike: first sprite, count, additive, texture.
type SpriteRun = (usize, usize, bool, *const Image);

/// Particles as camera-facing squares, farthest first: vertices (six per
/// sprite) and the runs to draw them in.
fn sprite_vertices(sprites: &[Sprite], camera: &Camera) -> (Vec<SpriteVertex>, Vec<SpriteRun>) {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut order: Vec<&Sprite> = sprites.iter().collect();
    let depth = |s: &Sprite| {
        dot(
            camera.forward,
            [s.position[0] - camera.eye[0], s.position[1] - camera.eye[1], s.position[2] - camera.eye[2]],
        )
    };
    order.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
    let mut vertices = Vec::with_capacity(order.len() * 6);
    let mut runs: Vec<SpriteRun> = Vec::new();
    for (i, s) in order.iter().enumerate() {
        let (sin, cos) = s.rotation.sin_cos();
        let (hw, hh) = (s.size[0] / 2.0, s.size[1] / 2.0);
        // The square's axes, turned in the view plane.
        let ax: [f32; 3] = std::array::from_fn(|k| (camera.right[k] * cos + camera.up[k] * sin) * hw);
        let ay: [f32; 3] = std::array::from_fn(|k| (camera.up[k] * cos - camera.right[k] * sin) * hh);
        let corner = |x: f32, y: f32, u: f32, v: f32| SpriteVertex {
            pos: std::array::from_fn(|k| s.position[k] + ax[k] * x + ay[k] * y),
            uv: [u, v],
            colour: s.colour,
        };
        let [u0, v0, u1, v1] = s.uv;
        let (a, b, c, d) = (
            corner(-1.0, 1.0, u0, v0),
            corner(1.0, 1.0, u1, v0),
            corner(1.0, -1.0, u1, v1),
            corner(-1.0, -1.0, u0, v1),
        );
        vertices.extend_from_slice(&[a, b, c, a, c, d]);
        let key = (s.additive, Arc::as_ptr(&s.texture));
        match runs.last_mut() {
            Some(run) if (run.2, run.3) == key => run.1 += 1,
            _ => runs.push((i, 1, key.0, key.1)),
        }
    }
    (vertices, runs)
}

/// What there is in a scene besides models.
#[derive(Default)]
pub struct Around<'a> {
    pub lights: &'a [SceneLight],
    pub sprites: &'a [Sprite],
    pub outdoors: Option<Outdoors>,
    /// The number of a view that is drawn again and again: it may get the
    /// picture before, which is much quicker.
    pub live: Option<u64>,
}

/// Renders several models (each with its own placement and pose) and
/// particles from `camera`, e.g. all actors of a cutscene.
pub fn render_scene(items: &[SceneItem], around: &Around, camera: &Camera, w: u32, h: u32) -> Option<slint::Image> {
    STATE.with(|state| {
        let mut st = state.borrow_mut();
        render_in(&mut st, items, around, camera, (w, h))
    })
}

fn render_in(
    st: &mut State,
    items: &[SceneItem],
    around: &Around,
    camera: &Camera,
    (w, h): (u32, u32),
) -> Option<slint::Image> {
    let &Around { lights, sprites, outdoors, live } = around;
    let gpu = GPU.as_ref()?;
    let (w, h) = (w.clamp(1, 8192), h.clamp(1, 8192));
    sync_resources(gpu, st, items, sprites, (w, h));
    let t = st.targets.as_mut()?;

    let globals = scene_globals(camera, lights, outdoors, (w, h));
    let globals_buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("globals"),
        contents: bytemuck::bytes_of(&globals),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let globals_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &gpu.globals_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buffer.as_entire_binding() }],
    });

    let cull = Cull::new(camera, outdoors, &globals.view_proj, (w, h));
    let shown = shown_draws(items, &st.models, &cull);
    let instances: Vec<Instance> = shown.iter().map(|(_, i)| *i).collect();
    let instance_buffer = vertex_buffer(gpu, "instances", &instances);

    let rest_pose = &*st.rest_pose.get_or_insert_with(|| palette_group(gpu, None));
    let skin_groups: Vec<Option<wgpu::BindGroup>> =
        items.iter().map(|item| item.skin.map(|s| palette_group(gpu, Some(s)))).collect();

    let (sprite_vertices, sprite_runs) = sprite_vertices(sprites, camera);
    let sprite_buffer = vertex_buffer(gpu, "sprites", &sprite_vertices);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let frame = Frame {
        gpu,
        outdoors,
        globals_group: &globals_group,
        instance_buffer: instance_buffer.as_ref(),
        shown: &shown,
        models: &st.models,
        skin_groups: &skin_groups,
        rest_pose,
        sprite_buffer: sprite_buffer.as_ref(),
        sprite_runs: &sprite_runs,
        sprite_textures: &st.sprite_textures,
    };
    record_pass(&mut encoder, t, &frame);
    let here = t.pending.as_ref().map_or(0, |(_, other, _)| 1 - other);
    let submitted = submit_readback(gpu, encoder, t, (w, h), here);
    let taken = take_picture(gpu, t, here, submitted, live)?;
    let pixels = read_pixels(t, taken, (w, h))?;
    if t.pending.as_ref().is_none_or(|(_, kept, _)| *kept != taken) {
        t.readback[taken].unmap();
    }
    Some(slint::Image::from_rgba8_premultiplied(pixels))
}

/// Uploads what the scene needs and is not there yet: models (forgetting
/// those that are gone), particle textures and the render targets.
fn sync_resources(gpu: &Gpu, st: &mut State, items: &[SceneItem], sprites: &[Sprite], (w, h): (u32, u32)) {
    // Upload once per model; forget models that are gone. A scene places
    // the same model many times: look models up by pointer, not by a scan
    // of every uploaded model for every placement.
    st.models.append(&mut READY.lock().unwrap_or_else(|e| e.into_inner()));
    st.models.retain(|m| m.owner.strong_count() > 0);
    let mut known: HashSet<*const crate::ModelPreview> = st.models.iter().map(|m| m.owner.as_ptr()).collect();
    let new: Vec<&Arc<crate::ModelPreview>> =
        items.iter().map(|item| item.preview).filter(|p| known.insert(Arc::as_ptr(p))).collect();
    if !new.is_empty() {
        st.models.extend(upload(gpu, new.into_iter().map(|p| build(gpu, p)).collect()));
    }

    st.sprite_textures.retain(|(owner, _)| owner.strong_count() > 0);
    for sprite in sprites {
        let ptr = Arc::as_ptr(&sprite.texture);
        if !st.sprite_textures.iter().any(|(o, _)| o.as_ptr() == ptr) {
            let t = &sprite.texture;
            let view = upload_texture(&gpu.device, &gpu.queue, t.width, t.height, &t.rgba);
            st.sprite_textures.push((Arc::downgrade(t), plain_bind_group(gpu, &view)));
        }
    }
    if st.targets.as_ref().is_none_or(|t| t.size != (w, h)) {
        st.targets = Some(targets(gpu, w, h));
    }
}

/// The lights of a scene in the layout the shader reads.
fn gpu_lights(lights: &[SceneLight]) -> [GpuLight; MAX_LIGHTS] {
    let mut gpu_lights = [<GpuLight as bytemuck::Zeroable>::zeroed(); MAX_LIGHTS];
    for (slot, l) in gpu_lights.iter_mut().zip(lights) {
        let (dir, cos) = match l.spot {
            Some((d, cone)) => (d, (cone.to_radians() / 2.0).cos()),
            None => ([0.0, 0.0, -1.0], -1.0),
        };
        *slot = GpuLight {
            position: [l.position[0], l.position[1], l.position[2], l.range],
            color: [l.color[0], l.color[1], l.color[2], if l.spot.is_some() { 1.0 } else { 0.0 }],
            direction: [dir[0], dir[1], dir[2], cos],
        };
    }
    gpu_lights
}

/// The shader's globals for one picture of `w` × `h`.
fn scene_globals(camera: &Camera, lights: &[SceneLight], outdoors: Option<Outdoors>, (w, h): (u32, u32)) -> Globals {
    let near = (camera.distance * 0.01).max(0.001);
    let far = camera.distance + camera.radius * 4.0 + 1.0;
    let gpu_lights = gpu_lights(lights);
    // How long the sides of the picture are, one away from the camera.
    let tan = (camera.fov / 2.0).tan();
    let short = w.min(h) as f32;
    let edge = |axis: [f32; 3], side: u32, last: f32| {
        let reach = tan * side as f32 / short;
        [axis[0] * reach, axis[1] * reach, axis[2] * reach, last]
    };
    let light = light_at(outdoors.map_or(12.0, |o| o.hour));
    let wide = |v: [f32; 3], last: f32| [v[0], v[1], v[2], last];
    Globals {
        view_proj: view_projection(camera, w, h, near, far),
        forward: [camera.forward[0], camera.forward[1], camera.forward[2], lights.len().min(MAX_LIGHTS) as f32],
        lights: gpu_lights,
        eye: [camera.eye[0], camera.eye[1], camera.eye[2], outdoors.map_or(0.0, |o| o.haze)],
        right: edge(camera.right, w, if outdoors.is_some() { 1.0 } else { 0.0 }),
        up: edge(camera.up, h, 0.0),
        sun: wide(light.towards, outdoors.map_or(0.0, |o| o.seconds)),
        sun_colour: wide(light.colour, light.lamps),
        ambient: wide(light.ambient, outdoors.map_or(1.0, |o| o.brightness)),
        zenith: wide(light.zenith, 0.0),
        horizon: wide(light.horizon, 0.0),
        below: wide(light.below, 0.0),
    }
}

/// What decides whether a draw is left out of a scene as too far or outside the view.
struct Cull<'a> {
    camera: &'a Camera,
    outdoors: Option<Outdoors>,
    planes: [[f32; 4]; 6],
    /// Pixels of something as large as it is far.
    pixels: f32,
}

impl<'a> Cull<'a> {
    fn new(camera: &'a Camera, outdoors: Option<Outdoors>, view_proj: &[f32; 16], (w, h): (u32, u32)) -> Self {
        let tan = (camera.fov / 2.0).tan();
        let short = w.min(h) as f32;
        Cull { camera, outdoors, planes: frustum_planes(view_proj), pixels: short / 2.0 / tan }
    }

    fn too_small(&self, model: &math::Mat4, (centre, radius): ([f32; 3], f32)) -> bool {
        let c = math::transform_point(model, centre);
        let far = (0..3).map(|k| (c[k] - self.camera.eye[k]).powi(2)).sum::<f32>().sqrt();
        radius * scale_of(model) * self.pixels < SMALLEST * (far - radius).max(0.0)
    }
}

// Whether it lies on others (those come last), the skinned item it is
// (or none), the model, the draw; the same ones are drawn together.
type Key = (bool, usize, usize, usize);
const NO_SKIN: usize = usize::MAX;

/// Whether a draw of `item` is in view and large enough to be worth drawing.
fn is_in_view(item: &SceneItem, d: &Draw, cull: &Cull) -> bool {
    if item.skin.is_some() {
        return true;
    }
    sphere_visible(&cull.planes, &item.model, d.sphere)
        && !(cull.outdoors.is_some() && cull.too_small(&item.model, d.sphere))
}

/// Shown draws of every item with their colours (hidden parts and
/// materials left out, and unskinned ones outside the view), sorted by key.
fn shown_draws(items: &[SceneItem], models: &[GpuModel], cull: &Cull) -> Vec<(Key, Instance)> {
    let model_of: HashMap<usize, usize> =
        models.iter().enumerate().map(|(i, m)| (m.owner.as_ptr() as usize, i)).collect();
    let mut shown: Vec<(Key, Instance)> = items
        .par_iter()
        .enumerate()
        .flat_map_iter(|(index, item)| {
            let number = model_of.get(&(Arc::as_ptr(item.preview) as usize)).copied();
            let draws = number.map_or(&[][..], |n| &models[n].draws[..]);
            let skin = if item.skin.is_some() { index } else { NO_SKIN };
            draws.iter().enumerate().filter_map(move |(k, d)| {
                if !item.look.parts.get(d.part).copied().unwrap_or(false) || !is_in_view(item, d, cull) {
                    return None;
                }
                let tint = item.preview.loaded.tint(item.look, d.drawable, d.shader)?;
                Some(((d.decal, skin, number?, k), Instance { model: item.model, tint }))
            })
        })
        .collect();
    shown.par_sort_unstable_by_key(|(key, _)| *key);
    shown
}

/// A vertex buffer holding `data`; `None` if there is nothing.
fn vertex_buffer<T: bytemuck::NoUninit>(gpu: &Gpu, label: &str, data: &[T]) -> Option<wgpu::Buffer> {
    (!data.is_empty()).then(|| {
        gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::VERTEX,
        })
    })
}

/// A bone palette per skinned item; the others share the rest pose.
fn palette_group(gpu: &Gpu, skin: Option<&[math::Mat4]>) -> wgpu::BindGroup {
    let mut palette = vec![math::IDENTITY; gpu.bones];
    for (slot, m) in palette.iter_mut().zip(skin.unwrap_or_default()) {
        *slot = *m;
    }
    let buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("skin"),
        contents: bytemuck::cast_slice(&palette),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &gpu.skin_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
    })
}

/// Everything the render pass of a scene draws from.
struct Frame<'a> {
    gpu: &'a Gpu,
    outdoors: Option<Outdoors>,
    globals_group: &'a wgpu::BindGroup,
    instance_buffer: Option<&'a wgpu::Buffer>,
    shown: &'a [(Key, Instance)],
    models: &'a [GpuModel],
    skin_groups: &'a [Option<wgpu::BindGroup>],
    rest_pose: &'a wgpu::BindGroup,
    sprite_buffer: Option<&'a wgpu::Buffer>,
    sprite_runs: &'a [SpriteRun],
    sprite_textures: &'a [(Weak<Image>, wgpu::BindGroup)],
}

fn record_pass(encoder: &mut wgpu::CommandEncoder, t: &Targets, frame: &Frame) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("model"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &t.msaa,
            depth_slice: None,
            resolve_target: Some(&t.resolve_view),
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Discard,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &t.depth,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_bind_group(0, frame.globals_group, &[]);
    if let Some(buffer) = frame.instance_buffer {
        draw_models(&mut pass, frame, buffer);
    }
    if frame.outdoors.is_some() {
        pass.set_pipeline(&frame.gpu.sky);
        pass.draw(0..3, 0..1);
        if frame.outdoors.is_some_and(|o| o.sea) {
            pass.set_pipeline(&frame.gpu.sea);
            pass.draw(0..4, 0..1);
        }
    }
    // Particles last, over what they are in front of.
    if let Some(buffer) = frame.sprite_buffer {
        draw_sprites(&mut pass, frame, buffer);
    }
}

fn draw_models(pass: &mut wgpu::RenderPass, frame: &Frame, buffer: &wgpu::Buffer) {
    pass.set_pipeline(&frame.gpu.pipeline);
    pass.set_vertex_buffer(1, buffer.slice(..));
    let mut last = (None, None, None);
    let mut first = 0;
    let mut decals = false;
    for run in frame.shown.chunk_by(|a, b| a.0 == b.0) {
        let (decal, skin, number, k) = run[0].0;
        if decal && !decals {
            pass.set_pipeline(&frame.gpu.decal);
            decals = true;
        }
        let model = &frame.models[number];
        let d = &model.draws[k];
        // Only what changes is set.
        if last.0 != Some(skin) {
            let group = frame.skin_groups.get(skin).and_then(Option::as_ref).unwrap_or(frame.rest_pose);
            pass.set_bind_group(2, group, &[]);
        }
        if last.1 != Some(model.batch) {
            pass.set_vertex_buffer(0, model.vertices.slice(..));
            pass.set_index_buffer(model.indices.slice(..), wgpu::IndexFormat::Uint32);
        }
        if last.2 != Some((number, d.texture)) {
            pass.set_bind_group(1, &model.textures[d.texture], &[]);
        }
        last = (Some(skin), Some(model.batch), Some((number, d.texture)));
        let count = run.len() as u32;
        pass.draw_indexed(d.indices.clone(), d.base_vertex, first..first + count);
        first += count;
    }
}

fn draw_sprites(pass: &mut wgpu::RenderPass, frame: &Frame, buffer: &wgpu::Buffer) {
    pass.set_vertex_buffer(0, buffer.slice(..));
    for &(first, count, additive, texture) in frame.sprite_runs {
        let Some((_, group)) = frame.sprite_textures.iter().find(|(o, _)| o.as_ptr() == texture) else {
            continue;
        };
        pass.set_pipeline(if additive { &frame.gpu.sprite_add } else { &frame.gpu.sprite_blend });
        pass.set_bind_group(1, group, &[]);
        let range = (first * 6) as u32..((first + count) * 6) as u32;
        pass.draw(range, 0..1);
    }
}

/// Copies the rendered picture into readback buffer `here` and submits.
fn submit_readback(
    gpu: &Gpu,
    mut encoder: wgpu::CommandEncoder,
    t: &Targets,
    (w, h): (u32, u32),
    here: usize,
) -> wgpu::SubmissionIndex {
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &t.resolve,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &t.readback[here],
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(t.padded_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()])
}

/// Waits for a picture and returns the readback buffer to take it from.
fn take_picture(
    gpu: &Gpu,
    t: &mut Targets,
    here: usize,
    submitted: wgpu::SubmissionIndex,
    live: Option<u64>,
) -> Option<usize> {
    t.readback[here].map_async(wgpu::MapMode::Read, .., |_| {});
    let wait = |index: &wgpu::SubmissionIndex| {
        let wait = wgpu::PollType::Wait { submission_index: Some(index.clone()), timeout: None };
        gpu.device.poll(wait).ok()
    };
    // While things move, the picture before is taken: it is there by now,
    // and the graphics card works while the next one is put together.
    let before = t.pending.take();
    match (live, before) {
        (Some(id), Some((of, other, index))) if of == id => {
            wait(&index)?;
            t.pending = Some((id, here, submitted));
            Some(other)
        }
        (_, before) => {
            wait(&submitted)?;
            if let Some((_, other, _)) = before {
                t.readback[other].unmap();
            }
            // The first of several is shown twice.
            t.pending = live.map(|id| (id, here, submitted));
            Some(here)
        }
    }
}

/// The pixels of readback buffer `taken`, without the row padding.
fn read_pixels(t: &Targets, taken: usize, (w, h): (u32, u32)) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    let data = t.readback[taken].get_mapped_range(..).ok()?;
    let out = pixels.make_mut_bytes();
    let row = (4 * w) as usize;
    for y in 0..h as usize {
        let src = y * t.padded_row as usize;
        out[y * row..(y + 1) * row].copy_from_slice(&data[src..src + row]);
    }
    drop(data);
    Some(pixels)
}

/// Releases all GPU resources. Must run before the program exits: wgpu's own
/// thread-local state is gone once thread-locals are destroyed.
pub fn shutdown() {
    let _ = STATE.try_with(|state| state.take());
}

#[cfg(test)]
mod tests {
    use wgpu::naga;

    #[test]
    fn culls_what_is_behind_or_beside_the_camera() {
        let camera = openrage_app::render::Camera {
            eye: [0.0; 3],
            forward: [0.0, 1.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            distance: 5.0,
            radius: 50.0,
            fov: 60f32.to_radians(),
        };
        let planes = super::frustum_planes(&super::view_projection(&camera, 800, 600, 0.05, 300.0));
        let at = |p: [f32; 3]| rage_formats::math::from_trs(p, [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let unit = ([0.0; 3], 1.0);
        assert!(super::sphere_visible(&planes, &at([0.0, 10.0, 0.0]), unit));
        assert!(!super::sphere_visible(&planes, &at([0.0, -10.0, 0.0]), unit));
        assert!(!super::sphere_visible(&planes, &at([40.0, 10.0, 0.0]), unit));
        // A large sphere beside the view still reaches into it.
        assert!(super::sphere_visible(&planes, &at([40.0, 10.0, 0.0]), ([0.0; 3], 40.0)));
    }

    #[test]
    fn the_sun_moves() {
        let height = |hour| super::light_at(hour).towards[2];
        assert!(height(12.0) > 0.8 && height(8.0) > 0.2);
        // At night there is a moon, and lamps.
        assert!(height(0.0) > 0.5);
        assert!(super::light_at(0.0).lamps > 1.0 && super::light_at(12.0).lamps == 0.0);
    }

    #[test]
    fn pictures_are_halved() {
        let (w, h, half) = super::halved(2, 2, &[0, 0, 0, 0, 100, 0, 0, 0, 100, 0, 0, 0, 200, 0, 0, 0]);
        assert_eq!((w, h, &half[..]), (1, 1, &[100, 0, 0, 0][..]));
        assert_eq!(super::halved(4, 1, &[7; 16]).2, [7; 8]);
    }

    #[test]
    fn sprites_draw_far_first_in_runs() {
        use openrage_app::gta4_effects::{Image, Sprite};
        use std::sync::Arc;
        let camera = openrage_app::render::Camera {
            eye: [0.0; 3],
            forward: [0.0, 1.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            distance: 5.0,
            radius: 50.0,
            fov: 60f32.to_radians(),
        };
        let image = Arc::new(Image { width: 1, height: 1, rgba: vec![255; 4] });
        let sprite = |y: f32, additive: bool| Sprite {
            position: [0.0, y, 0.0],
            size: [2.0, 2.0],
            rotation: 0.0,
            colour: [1.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            texture: Arc::clone(&image),
            additive,
        };
        let (vertices, runs) =
            super::sprite_vertices(&[sprite(1.0, false), sprite(5.0, false), sprite(3.0, true)], &camera);
        assert_eq!(vertices.len(), 18);
        // Farthest first: y 5 and 3, then 1; alike neighbours share a run.
        assert_eq!(vertices[0].pos[1], 5.0);
        assert_eq!(
            runs.iter().map(|r| (r.0, r.1, r.2)).collect::<Vec<_>>(),
            [(0, 1, false), (1, 1, true), (2, 1, false)]
        );
        // A 2 m square around its centre, facing the camera.
        let xs: Vec<f32> = vertices[..6].iter().map(|v| v.pos[0]).collect();
        assert_eq!(
            (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max)),
            (-1.0, 1.0)
        );
    }

    #[test]
    fn shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(&super::shader_source(super::MAX_BONES)).expect("WGSL parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("WGSL validates");
    }
}
