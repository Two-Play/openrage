//! GPU renderer for model previews (wgpu: Vulkan, DirectX 12, Metal or GL).
//!
//! Renders offscreen with 4× multisampling and hands the image to Slint, so it
//! works whatever Slint itself draws with. Meshes are uploaded once per model
//! (with each part's placement baked in); a frame only updates the camera.
//! If no GPU is available, callers fall back to the software renderer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use openrage_app::models::{LoadedModel, Look};
use openrage_app::render::{Camera, Materials};
use rage_formats::math;
use wgpu::util::DeviceExt;

const SAMPLES: u32 = 4;
/// Bones the skinning matrices cover (16 KiB of uniforms, the downlevel limit).
const MAX_BONES: usize = 256;
/// Bytes of per-draw data: colour (vec4) and model matrix (mat4).
const DRAW_DATA: usize = 16 + 64;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    // xyz: viewing direction (headlight)
    forward: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
// Overlay drawn over the base colour with its alpha, on the second UV set
// (vehicle liveries and signs); transparent when there is none.
@group(1) @binding(2) var overlay: texture_2d<f32>;
// Per draw: material colour (multiplied with the texture) and the model's
// placement in the scene.
struct DrawData {
    tint: vec4<f32>,
    model: mat4x4<f32>,
};
@group(2) @binding(0) var<uniform> draw: DrawData;
// Skinning matrices per skeleton bone (identity when not animated).
@group(3) @binding(0) var<uniform> skin: array<mat4x4<f32>, 256>;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) uv1: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) uv1: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    var pos = vec4<f32>(v.pos, 1.0);
    var normal = v.normal;
    let total = v.weights.x + v.weights.y + v.weights.z + v.weights.w;
    if (total > 0.0) {
        let m = (skin[min(v.joints.x, 255u)] * v.weights.x + skin[min(v.joints.y, 255u)] * v.weights.y
            + skin[min(v.joints.z, 255u)] * v.weights.z + skin[min(v.joints.w, 255u)] * v.weights.w) * (1.0 / total);
        pos = m * pos;
        normal = (m * vec4<f32>(normal, 0.0)).xyz;
    }
    o.clip = globals.view_proj * draw.model * pos;
    o.normal = (draw.model * vec4<f32>(normal, 0.0)).xyz;
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
    // Headlight plus a little sky light; double-sided.
    let light = 0.25 + 0.65 * abs(dot(n, globals.forward.xyz)) + 0.1 * max(n.z, 0.0);
    let layer = textureSample(overlay, samp, i.uv1);
    let colour = mix(base.rgb * draw.tint.rgb, layer.rgb, layer.a);
    return vec4<f32>(colour * light, 1.0);
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
struct Globals {
    view_proj: [f32; 16],
    forward: [f32; 4],
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    tint_layout: wgpu::BindGroupLayout,
    skin_layout: wgpu::BindGroupLayout,
    /// Stride of the per-draw colours in the tint buffer.
    tint_stride: u64,
    sampler: wgpu::Sampler,
    /// White 1×1 texture for untextured meshes (their colour is the tint).
    white: wgpu::TextureView,
    /// Transparent 1×1 texture for meshes without an overlay.
    clear: wgpu::TextureView,
}

struct Draw {
    part: usize,
    drawable: usize,
    shader: u16,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    texture: usize,
}

/// A model uploaded to the GPU.
struct GpuModel {
    owner: Weak<crate::ModelPreview>,
    draws: Vec<Draw>,
    /// Bind groups: 0 is the plain texture, then one per pair of decoded
    /// images (base colour, overlay).
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
    gpu: Option<Option<Gpu>>,
    models: Vec<GpuModel>,
    targets: Option<Targets>,
}

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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("openrage"),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .ok()?;

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("model"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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
        ],
    });
    let tint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tint"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(DRAW_DATA as u64),
            },
            count: None,
        }],
    });
    let align = (device.limits().min_uniform_buffer_offset_alignment as u64).max(16);
    let tint_stride = (DRAW_DATA as u64).div_ceil(align) * align;
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
        bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout), Some(&tint_layout), Some(&skin_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4, 4 => Uint32x4, 5 => Float32x2],
            })],
        },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
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
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let white = upload_texture(&device, &queue, 1, 1, &[255, 255, 255, 255]);
    let clear = upload_texture(&device, &queue, 1, 1, &[0, 0, 0, 0]);
    Some(Gpu {
        device,
        queue,
        pipeline,
        globals_layout,
        texture_layout,
        tint_layout,
        skin_layout,
        tint_stride,
        sampler,
        white,
        clear,
    })
}

/// Uploads RGBA pixels as a texture.
fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Colors are passed through unchanged, like the software renderer.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
        size,
    );
    texture.create_view(&Default::default())
}

/// A material's textures: base colour and overlay.
fn texture_bind_group(gpu: &Gpu, base: &wgpu::TextureView, overlay: &wgpu::TextureView) -> wgpu::BindGroup {
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &gpu.texture_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(base) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(overlay) },
        ],
    })
}

/// Uploads every part of the model (placements baked into the vertices).
fn upload(gpu: &Gpu, model: &LoadedModel, materials: &Materials) -> (Vec<Draw>, Vec<wgpu::BindGroup>) {
    let mut textures = vec![texture_bind_group(gpu, &gpu.white, &gpu.clear)];
    // Material image index → uploaded texture; (base, overlay) image indices → bind group.
    let mut views: HashMap<usize, wgpu::TextureView> = HashMap::new();
    let mut texture_of: HashMap<(Option<usize>, Option<usize>), usize> = HashMap::new();
    texture_of.insert((None, None), 0);
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
            let mut view = |image: Option<(usize, usize, usize, &[u8])>| {
                image.map(|(i, w, h, rgba)| {
                    views.entry(i).or_insert_with(|| upload_texture(&gpu.device, &gpu.queue, w as u32, h as u32, rgba));
                    i
                })
            };
            let key = (view(base), view(overlay));
            let texture = *texture_of.entry(key).or_insert_with(|| {
                let base = key.0.map_or(&gpu.white, |i| &views[&i]);
                let overlay = key.1.map_or(&gpu.clear, |i| &views[&i]);
                textures.push(texture_bind_group(gpu, base, overlay));
                textures.len() - 1
            });
            draws.push(Draw {
                part: part_index,
                drawable: part.drawable,
                shader: mesh.shader,
                vertices: gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                count: mesh.indices.len() as u32,
                texture,
            });
        }
    }
    (draws, textures)
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
    // Perspective with depth 0..1 (wgpu).
    let a = far / (near - far);
    let proj = [sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, a, -1.0, 0.0, 0.0, a * near, 0.0];
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
    render_scene(&[SceneItem { preview, look, model: math::IDENTITY, skin }], camera, w, h)
}

/// One model of a scene: what is shown, where it stands and its pose.
pub struct SceneItem<'a> {
    pub preview: &'a Arc<crate::ModelPreview>,
    pub look: &'a Look,
    /// Placement in the scene.
    pub model: math::Mat4,
    pub skin: Option<&'a [math::Mat4]>,
}

/// Renders several models (each with its own placement and pose) from
/// `camera`, e.g. all actors of a cutscene.
pub fn render_scene(items: &[SceneItem], camera: &Camera, w: u32, h: u32) -> Option<slint::Image> {
    STATE.with(|state| {
        let mut st = state.borrow_mut();
        let st = &mut *st;
        if st.gpu.is_none() {
            st.gpu = Some(init());
        }
        let gpu = st.gpu.as_ref()?.as_ref()?;
        let (w, h) = (w.clamp(1, 8192), h.clamp(1, 8192));

        // Upload once per model; forget models that are gone.
        st.models.retain(|m| m.owner.strong_count() > 0);
        for item in items {
            if !st.models.iter().any(|m| std::ptr::eq(m.owner.as_ptr(), Arc::as_ptr(item.preview))) {
                let (draws, textures) = upload(gpu, &item.preview.loaded, &item.preview.materials);
                st.models.push(GpuModel { owner: Arc::downgrade(item.preview), draws, textures });
            }
        }

        if st.targets.as_ref().is_none_or(|t| t.size != (w, h)) {
            st.targets = Some(targets(gpu, w, h));
        }
        let t = st.targets.as_ref()?;

        let near = (camera.distance * 0.01).max(0.001);
        let far = camera.distance + camera.radius * 4.0 + 1.0;
        let globals = Globals {
            view_proj: view_projection(camera, w, h, near, far),
            forward: [camera.forward[0], camera.forward[1], camera.forward[2], 0.0],
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

        // Shown draws of every item with their colours (hidden parts and materials left out).
        let mut shown: Vec<(usize, &GpuModel, &Draw, [f32; 4])> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let model = st.models.iter().find(|m| std::ptr::eq(m.owner.as_ptr(), Arc::as_ptr(item.preview)))?;
            for d in &model.draws {
                if !item.look.parts.get(d.part).copied().unwrap_or(false) {
                    continue;
                }
                if let Some(tint) = item.preview.loaded.tint(item.look, d.drawable, d.shader) {
                    shown.push((index, model, d, tint));
                }
            }
        }
        let stride = gpu.tint_stride as usize;
        let mut tints = vec![0u8; stride * shown.len().max(1)];
        for (i, (index, _, _, tint)) in shown.iter().enumerate() {
            tints[i * stride..i * stride + 16].copy_from_slice(bytemuck::cast_slice(tint));
            tints[i * stride + 16..i * stride + DRAW_DATA].copy_from_slice(bytemuck::cast_slice(&items[*index].model));
        }
        let tint_buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("draws"),
            contents: &tints,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let tint_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &gpu.tint_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &tint_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(DRAW_DATA as u64),
                }),
            }],
        });

        // One bone palette per item.
        let skin_groups: Vec<wgpu::BindGroup> = items
            .iter()
            .map(|item| {
                let mut palette = vec![math::IDENTITY; MAX_BONES];
                if let Some(skin) = item.skin {
                    for (slot, m) in palette.iter_mut().zip(skin) {
                        *slot = *m;
                    }
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
            })
            .collect();

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
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &globals_group, &[]);
            for (i, (index, model, d, _)) in shown.iter().enumerate() {
                pass.set_bind_group(3, &skin_groups[*index], &[]);
                pass.set_bind_group(1, &model.textures[d.texture], &[]);
                pass.set_bind_group(2, &tint_group, &[(i * stride) as u32]);
                pass.set_vertex_buffer(0, d.vertices.slice(..));
                pass.set_index_buffer(d.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..d.count, 0, 0..1);
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
    fn shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(super::SHADER).expect("WGSL parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("WGSL validates");
    }
}
