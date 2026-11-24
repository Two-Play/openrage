//! GPU renderer for model previews (wgpu: Vulkan, DirectX 12, Metal or GL).
//!
//! Renders offscreen with 4× multisampling and hands the image to Slint, so it
//! works whatever Slint itself draws with. Meshes are uploaded once per model
//! (with each part's placement baked in); a frame only updates the camera.
//! If no GPU is available, callers fall back to the software renderer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use openrage_app::gta4_effects::{Image, Sprite};
use openrage_app::models::Look;
use openrage_app::render::Camera;
use rage_formats::math;
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

// Where the sun is, and the colours of the sky.
const SUN: vec3<f32> = vec3<f32>(0.48, -0.42, 0.77);
const ZENITH: vec3<f32> = vec3<f32>(0.22, 0.45, 0.78);
const HORIZON: vec3<f32> = vec3<f32>(0.74, 0.82, 0.89);
const BELOW: vec3<f32> = vec3<f32>(0.40, 0.48, 0.55);

fn sky_colour(dir: vec3<f32>) -> vec3<f32> {
    let sun = normalize(SUN);
    var c = mix(HORIZON, ZENITH, pow(clamp(dir.z, 0.0, 1.0), 0.55));
    c = mix(c, BELOW, smoothstep(0.0, 0.25, -dir.z));
    let near = max(dot(dir, sun), 0.0);
    return c + vec3<f32>(1.0, 0.92, 0.75) * (pow(near, 900.0) + 0.12 * pow(near, 12.0));
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
    return vec4<f32>(sky_colour(normalize(i.dir)), 1.0);
}

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) uv1: vec2<f32>,
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
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    var pos = vec4<f32>(v.pos, 1.0);
    var normal = v.normal;
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
    o.uv = v.uv;
    o.uv1 = v.uv1;
    return o;
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(tex, samp, i.uv);
    if (base.a < 0.3) {
        discard;
    }
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
        light = vec3<f32>(1.0, 0.95, 0.86) * 0.75 * max(dot(m, normalize(SUN)), 0.0)
            + vec3<f32>(0.48, 0.53, 0.60) * (0.85 + 0.35 * m.z);
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
    let colour = mix(base.rgb * i.tint, layer.rgb, layer.a);
    // What shines towards the viewer.
    let glint = pow(abs(dot(n, globals.forward.xyz)), 24.0) * max(max(shine.r, shine.g), shine.b) * 0.25;
    var lit = colour * light + vec3<f32>(glint);
    if (globals.right.w > 0.5) {
        let ray = i.world - globals.eye.xyz;
        let far = length(ray);
        let dir = ray / max(far, 0.001);
        let haze = sky_colour(normalize(vec3<f32>(dir.xy, max(dir.z, 0.0))));
        lit = mix(haze, colour * light, exp(-far * globals.eye.w));
    }
    return vec4<f32>(lit, 1.0);
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
    normal: [f32; 3],
    uv: [f32; 2],
    weights: [f32; 4],
    joints: [u32; 4],
    uv1: [f32; 2],
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
}

/// Outdoors, models smaller than this in the picture are left out (pixels).
const SMALLEST: f32 = 1.2;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// Particles blended over the scene, and particles that add light.
    sprite_blend: wgpu::RenderPipeline,
    sprite_add: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    skin_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// White 1×1 texture for untextured meshes (their colour is the tint).
    white: wgpu::TextureView,
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
    /// Bounding sphere (centre, radius) in the model's space, for culling.
    sphere: ([f32; 3], f32),
}

/// A model uploaded to the GPU.
struct GpuModel {
    owner: Weak<crate::ModelPreview>,
    draws: Vec<Draw>,
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
    readback: wgpu::Buffer,
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("openrage"),
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
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4, 4 => Uint32x4, 5 => Float32x2],
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
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky"),
        bind_group_layouts: &[Some(&globals_layout)],
        immediate_size: 0,
    });
    let sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: Some(&sky_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
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
            entry_point: Some("fs_sky"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
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
    let clear = upload_texture(&device, &queue, 1, 1, &[0, 0, 0, 0]);
    let flat = upload_texture(&device, &queue, 1, 1, &[128, 128, 255, 255]);
    Some(Gpu {
        sprite_blend,
        sprite_add,
        sky,
        device,
        queue,
        pipeline,
        globals_layout,
        texture_layout,
        skin_layout,
        sampler,
        white,
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

/// Uploads every part of the model (placements baked into the vertices).
fn upload(gpu: &Gpu, preview: &Arc<crate::ModelPreview>) -> GpuModel {
    let (model, materials) = (&preview.loaded, &preview.materials);
    let (mut all_vertices, mut all_indices): (Vec<Vertex>, Vec<u32>) = (Vec::new(), Vec::new());
    let mut textures = vec![plain_bind_group(gpu, &gpu.white)];
    // Material image index → uploaded texture; image indices → bind group.
    let mut views: HashMap<usize, wgpu::TextureView> = HashMap::new();
    let mut texture_of: HashMap<[Option<usize>; 4], usize> = HashMap::new();
    texture_of.insert([None; 4], 0);
    let mut draws = Vec::new();
    for (part_index, part) in model.parts.iter().enumerate() {
        let Some((_, d)) = model.drawables.get(part.drawable) else { continue };
        for mesh in &d.meshes {
            if mesh.positions.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            let has_n = mesh.normals.len() == mesh.positions.len();
            let has_uv = mesh.uvs.len() == mesh.positions.len();
            let has_uv1 = mesh.uvs1.len() == mesh.positions.len();
            let skinned = mesh.bone_weights.len() == mesh.positions.len();
            let bone = part.bone.map(|b| b as u32);
            let vertices: Vec<Vertex> = (0..mesh.positions.len())
                .map(|i| Vertex {
                    pos: math::transform_point(&part.transform, mesh.positions[i]),
                    normal: if has_n { math::transform_dir(&part.transform, mesh.normals[i]) } else { [0.0; 3] },
                    uv: if has_uv { mesh.uvs[i] } else { [0.0; 2] },
                    // Parts attached to a bone follow it; others use the mesh's weights.
                    weights: match bone {
                        Some(_) => [1.0, 0.0, 0.0, 0.0],
                        None if skinned => mesh.bone_weights[i],
                        None => [0.0; 4],
                    },
                    joints: match bone {
                        Some(b) => [b, 0, 0, 0],
                        None if skinned => mesh.bone_indices[i].map(u32::from),
                        None => [0; 4],
                    },
                    uv1: if has_uv1 { mesh.uvs1[i] } else { [0.0; 2] },
                })
                .collect();
            let base = materials.texture(part.drawable, mesh.shader).filter(|_| has_uv);
            let overlay = materials.overlay(part.drawable, mesh.shader).filter(|_| has_uv1);
            let normal = materials.normal(part.drawable, mesh.shader).filter(|_| has_uv);
            let specular = materials.specular(part.drawable, mesh.shader).filter(|_| has_uv);
            let mut view = |image: Option<(usize, usize, usize, &[u8])>| {
                image.map(|(i, w, h, rgba)| {
                    views.entry(i).or_insert_with(|| upload_texture(&gpu.device, &gpu.queue, w as u32, h as u32, rgba));
                    i
                })
            };
            let key = [view(base), view(overlay), view(normal), view(specular)];
            let texture = *texture_of.entry(key).or_insert_with(|| {
                let [base, overlay, normal, specular] = key;
                let of = |image: Option<usize>, none| image.map_or(none, |i| &views[&i]);
                textures.push(texture_bind_group(
                    gpu,
                    of(base, &gpu.white),
                    of(overlay, &gpu.clear),
                    of(normal, &gpu.flat),
                    of(specular, &gpu.clear),
                ));
                textures.len() - 1
            });
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &vertices {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.pos[k]);
                    hi[k] = hi[k].max(v.pos[k]);
                }
            }
            let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
            let radius = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
            draws.push(Draw {
                sphere: (centre, radius),
                part: part_index,
                drawable: part.drawable,
                shader: mesh.shader,
                base_vertex: all_vertices.len() as i32,
                indices: all_indices.len() as u32..(all_indices.len() + mesh.indices.len()) as u32,
                texture,
            });
            all_vertices.extend_from_slice(&vertices);
            all_indices.extend_from_slice(&mesh.indices);
        }
    }
    // An empty buffer cannot be used.
    if all_indices.is_empty() {
        all_vertices.push(bytemuck::Zeroable::zeroed());
        all_indices.push(0);
    }
    let buffer = |contents: &[u8], usage| {
        gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents, usage })
    };
    GpuModel {
        owner: Arc::downgrade(preview),
        draws,
        vertices: buffer(bytemuck::cast_slice(&all_vertices), wgpu::BufferUsages::VERTEX),
        indices: buffer(bytemuck::cast_slice(&all_indices), wgpu::BufferUsages::INDEX),
        textures,
    }
}

/// Uploads a model ahead of its first picture; from any thread.
pub fn prepare(preview: &Arc<crate::ModelPreview>) {
    if let Some(gpu) = &*GPU {
        let model = upload(gpu, preview);
        READY.lock().unwrap_or_else(|e| e.into_inner()).push(model);
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
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Targets {
        size: (w, h),
        msaa: msaa.create_view(&Default::default()),
        resolve_view: resolve.create_view(&Default::default()),
        resolve,
        depth: depth.create_view(&Default::default()),
        readback,
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
    render_scene(&[SceneItem { preview, look, model: math::IDENTITY, skin }], &[], &[], None, camera, w, h)
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

/// Renders several models (each with its own placement and pose) and
/// particles from `camera`, e.g. all actors of a cutscene.
pub fn render_scene(
    items: &[SceneItem],
    lights: &[SceneLight],
    sprites: &[Sprite],
    outdoors: Option<Outdoors>,
    camera: &Camera,
    w: u32,
    h: u32,
) -> Option<slint::Image> {
    STATE.with(|state| {
        let mut st = state.borrow_mut();
        let st = &mut *st;
        let gpu = GPU.as_ref()?;
        let (w, h) = (w.clamp(1, 8192), h.clamp(1, 8192));

        // Upload once per model; forget models that are gone. A scene places
        // the same model many times: look models up by pointer, not by a scan
        // of every uploaded model for every placement.
        st.models.append(&mut READY.lock().unwrap_or_else(|e| e.into_inner()));
        st.models.retain(|m| m.owner.strong_count() > 0);
        let mut uploaded: std::collections::HashSet<*const crate::ModelPreview> =
            st.models.iter().map(|m| m.owner.as_ptr()).collect();
        for item in items {
            if uploaded.insert(Arc::as_ptr(item.preview)) {
                st.models.push(upload(gpu, item.preview));
            }
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
        let t = st.targets.as_ref()?;

        let near = (camera.distance * 0.01).max(0.001);
        let far = camera.distance + camera.radius * 4.0 + 1.0;
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
        // How long the sides of the picture are, one away from the camera.
        let tan = (camera.fov / 2.0).tan();
        let short = w.min(h) as f32;
        let edge = |axis: [f32; 3], side: u32, last: f32| {
            let reach = tan * side as f32 / short;
            [axis[0] * reach, axis[1] * reach, axis[2] * reach, last]
        };
        let globals = Globals {
            view_proj: view_projection(camera, w, h, near, far),
            forward: [camera.forward[0], camera.forward[1], camera.forward[2], lights.len().min(MAX_LIGHTS) as f32],
            lights: gpu_lights,
            eye: [camera.eye[0], camera.eye[1], camera.eye[2], outdoors.map_or(0.0, |o| o.haze)],
            right: edge(camera.right, w, if outdoors.is_some() { 1.0 } else { 0.0 }),
            up: edge(camera.up, h, 0.0),
        };
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

        // Shown draws of every item with their colours (hidden parts and
        // materials left out, and unskinned ones outside the view).
        let model_of: HashMap<*const crate::ModelPreview, usize> =
            st.models.iter().enumerate().map(|(i, m)| (m.owner.as_ptr(), i)).collect();
        let planes = frustum_planes(&globals.view_proj);
        // Pixels of something as large as it is far.
        let pixels = short / 2.0 / tan;
        let too_small = |model: &math::Mat4, (centre, radius): ([f32; 3], f32)| {
            let c = math::transform_point(model, centre);
            let far = (0..3).map(|k| (c[k] - camera.eye[k]).powi(2)).sum::<f32>().sqrt();
            radius * scale_of(model) * pixels < SMALLEST * (far - radius).max(0.0)
        };
        // The skinned item it is (or none), the model, the draw; the same
        // ones are drawn together.
        const NO_SKIN: usize = usize::MAX;
        let mut shown: Vec<((usize, usize, usize), Instance)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let number = *model_of.get(&Arc::as_ptr(item.preview))?;
            for (k, d) in st.models[number].draws.iter().enumerate() {
                if !item.look.parts.get(d.part).copied().unwrap_or(false) {
                    continue;
                }
                if item.skin.is_none()
                    && (!sphere_visible(&planes, &item.model, d.sphere)
                        || (outdoors.is_some() && too_small(&item.model, d.sphere)))
                {
                    continue;
                }
                if let Some(tint) = item.preview.loaded.tint(item.look, d.drawable, d.shader) {
                    let skin = if item.skin.is_some() { index } else { NO_SKIN };
                    shown.push(((skin, number, k), Instance { model: item.model, tint }));
                }
            }
        }
        shown.sort_by_key(|(key, _)| *key);
        let instances: Vec<Instance> = shown.iter().map(|(_, i)| *i).collect();
        let instance_buffer = (!instances.is_empty()).then(|| {
            gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        // A bone palette per skinned item; the others share the rest pose.
        let palette_group = |skin: Option<&[math::Mat4]>| {
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
        };
        let rest_pose = &*st.rest_pose.get_or_insert_with(|| palette_group(None));
        let skin_groups: Vec<Option<wgpu::BindGroup>> =
            items.iter().map(|item| item.skin.map(|s| palette_group(Some(s)))).collect();

        let (sprite_vertices, sprite_runs) = sprite_vertices(sprites, camera);
        let sprite_buffer = (!sprite_vertices.is_empty()).then(|| {
            gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sprites"),
                contents: bytemuck::cast_slice(&sprite_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
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
            pass.set_bind_group(0, &globals_group, &[]);
            if let Some(buffer) = &instance_buffer {
                pass.set_pipeline(&gpu.pipeline);
                pass.set_vertex_buffer(1, buffer.slice(..));
                let mut last = (None, None, None);
                let mut first = 0;
                for run in shown.chunk_by(|a, b| a.0 == b.0) {
                    let (skin, number, k) = run[0].0;
                    let model = &st.models[number];
                    let d = &model.draws[k];
                    // Only what changes is set.
                    if last.0 != Some(skin) {
                        let group = skin_groups.get(skin).and_then(Option::as_ref).unwrap_or(rest_pose);
                        pass.set_bind_group(2, group, &[]);
                    }
                    if last.1 != Some(number) {
                        pass.set_vertex_buffer(0, model.vertices.slice(..));
                        pass.set_index_buffer(model.indices.slice(..), wgpu::IndexFormat::Uint32);
                    }
                    if last.2 != Some((number, d.texture)) {
                        pass.set_bind_group(1, &model.textures[d.texture], &[]);
                    }
                    last = (Some(skin), Some(number), Some((number, d.texture)));
                    let count = run.len() as u32;
                    pass.draw_indexed(d.indices.clone(), d.base_vertex, first..first + count);
                    first += count;
                }
            }
            if outdoors.is_some() {
                pass.set_pipeline(&gpu.sky);
                pass.draw(0..3, 0..1);
            }
            // Particles last, over what they are in front of.
            if let Some(buffer) = &sprite_buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                for &(first, count, additive, texture) in &sprite_runs {
                    let Some((_, group)) = st.sprite_textures.iter().find(|(o, _)| o.as_ptr() == texture) else {
                        continue;
                    };
                    pass.set_pipeline(if additive { &gpu.sprite_add } else { &gpu.sprite_blend });
                    pass.set_bind_group(1, group, &[]);
                    let range = (first * 6) as u32..((first + count) * 6) as u32;
                    pass.draw(range, 0..1);
                }
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &t.resolve,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &t.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(t.padded_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);

        t.readback.map_async(wgpu::MapMode::Read, .., |_| {});
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        {
            let data = t.readback.get_mapped_range(..).ok()?;
            let out = pixels.make_mut_bytes();
            let row = (4 * w) as usize;
            for y in 0..h as usize {
                let src = y * t.padded_row as usize;
                out[y * row..(y + 1) * row].copy_from_slice(&data[src..src + row]);
            }
        }
        t.readback.unmap();
        Some(slint::Image::from_rgba8_premultiplied(pixels))
    })
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
