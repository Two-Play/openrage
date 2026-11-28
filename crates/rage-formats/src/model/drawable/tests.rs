use super::*;

#[test]
fn parameter_hashes() {
    use rage_core::hash::joaat;
    for (name, hash) in [
        ("DiffuseTex", param::DIFFUSE_TEX),
        ("DiffuseSampler", param::DIFFUSE_SAMPLER),
        ("DiffuseTex2", param::DIFFUSE_TEX2),
        ("DiffuseSampler2", param::DIFFUSE_SAMPLER2),
        ("DirtTex", param::DIRT_TEX),
        ("SpecularTex", param::SPECULAR_TEX),
        ("BumpTex", param::BUMP_TEX),
        ("DamageTexture", param::DAMAGE_TEXTURE),
        ("DirtSampler", param::DIRT_SAMPLER),
        ("SpecSampler", param::SPEC_SAMPLER),
        ("BumpSampler", param::BUMP_SAMPLER),
    ] {
        assert_eq!(joaat(name), hash, "{name}");
    }
}

#[test]
fn base_texture_by_parameter() {
    let s = |samplers: &[(u32, &str)]| Shader {
        name_hash: 0,
        textures: samplers.iter().map(|(_, n)| n.to_string()).collect(),
        samplers: samplers.iter().map(|(h, n)| (*h, n.to_string())).collect(),
    };
    // The livery in DiffuseTex2 is not the base colour.
    let paint = s(&[
        (param::DIFFUSE_TEX, "vehicle_generic_smallspecmap"),
        (param::DIFFUSE_TEX2, "ambulance_sign_1"),
        (param::DIRT_TEX, "vehicle_genericmud_car"),
    ]);
    // Not paint: DiffuseTex is the base colour.
    assert_eq!(paint.base_texture(), Some("vehicle_generic_smallspecmap"));
    assert_eq!(paint.overlay_texture(), Some("ambulance_sign_1"));
    // Paint: the placeholder in DiffuseTex is no colour texture.
    let paint = Shader { name_hash: rage_core::hash::joaat("vehicle_paint3"), ..paint };
    assert_eq!(paint.base_texture(), None);
    assert_eq!(paint.overlay_texture(), Some("ambulance_sign_1"));
    // Without DiffuseTex, a texture bound only to DiffuseTex2 is skipped.
    let livery_only = s(&[(param::DIFFUSE_TEX2, "airbus_sign_1"), (0, "airbus_body")]);
    assert_eq!(livery_only.base_texture(), Some("airbus_body"));
    // Unknown parameters: by name.
    let unknown = s(&[(0, "plate01_n"), (0, "plate01")]);
    assert_eq!(unknown.base_texture(), Some("plate01"));
    // RDR2 palette shaders: the shade in lyr0DiffuseTex, not the palette.
    let pal = s(&[
        (param::TINT_PALETTE_TEX, "w_repeater_cloth_strap01_pal"),
        (param::LYR0_DIFFUSE_TEX, "w_repeater_cloth_strap01_ab"),
    ]);
    assert_eq!(pal.base_texture(), Some("w_repeater_cloth_strap01_ab"));
}

#[test]
fn half_floats() {
    let h = |bits: u16| Encoding::Half2.decode(&bits.to_le_bytes())[0];
    assert_eq!(h(0x3C00), 1.0);
    assert_eq!(h(0xC000), -2.0);
    assert_eq!(h(0x0000), 0.0);
    assert!((h(0x3555) - 0.333_25).abs() < 1e-4);
    // A short slice must not panic.
    assert_eq!(Encoding::Float4.decode(&[0, 0, 0x80, 0x3F]), [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn packed_normals() {
    let v: u32 = 511 | ((1024 - 511) << 20);
    let n = Encoding::R10G10B10A2.decode(&v.to_le_bytes());
    assert_eq!([n[0], n[1], n[2]], [1.0, 0.0, -1.0]);
}

const SYS: u64 = 0x5000_0000;

/// Flat system memory with a bump allocator; the root structure lives at its start.
struct Mem(Vec<u8>);

impl Mem {
    fn new() -> Self {
        Mem(vec![0; 0x200])
    }

    fn alloc(&mut self, len: usize) -> u64 {
        let at = SYS + self.0.len() as u64;
        self.0.resize(self.0.len() + len.max(1).next_multiple_of(16), 0);
        at
    }

    fn bytes(&mut self, b: &[u8]) -> u64 {
        let at = self.alloc(b.len());
        self.put(at, b);
        at
    }

    fn string(&mut self, s: &str) -> u64 {
        self.bytes(&[s.as_bytes(), &[0]].concat())
    }

    fn put(&mut self, at: u64, b: &[u8]) {
        let o = (at - SYS) as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
    }

    fn u8(&mut self, at: u64, v: u8) {
        self.put(at, &[v]);
    }

    fn u16(&mut self, at: u64, v: u16) {
        self.put(at, &v.to_le_bytes());
    }

    fn u32(&mut self, at: u64, v: u32) {
        self.put(at, &v.to_le_bytes());
    }

    fn ptr(&mut self, at: u64, v: u64) {
        self.put(at, &v.to_le_bytes());
    }

    fn f32s(&mut self, at: u64, v: &[f32]) {
        for (k, x) in v.iter().enumerate() {
            self.put(at + k as u64 * 4, &x.to_le_bytes());
        }
    }

    fn res(self, version: u32) -> Resource {
        Resource { version, system: self.0, graphics: Vec::new() }
    }
}

/// A Direct3D-style vertex declaration of Enhanced: `(slot, offset, type)` for each attribute.
fn gen9_decl(m: &mut Mem, slots: &[(usize, u32, u8)]) -> u64 {
    let decl = m.alloc(52 * 4 + 52 + 52 + 12);
    for &(slot, offset, t) in slots {
        m.u32(decl + slot as u64 * 4, offset);
        m.u8(decl + 52 * 4 + 52 + slot as u64, t);
    }
    decl
}

fn legacy_decl(m: &mut Mem, flags: u32, types: u64) -> u64 {
    let decl = m.alloc(16);
    m.u32(decl, flags);
    m.ptr(decl + 8, types);
    decl
}

/// A resource holding only a declaration, to test layouts.
fn layout_of(slots: &[(usize, u32, u8)]) -> VertexLayout {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, slots);
    gen9_layout(&m.res(159), decl, 32).unwrap()
}

#[test]
fn enhanced_declarations_name_their_attributes() {
    let l = layout_of(&[(0, 0, 6), (4, 12, 24), (24, 16, 28), (28, 20, 34), (29, 24, 16), (16, 28, 28), (20, 32, 30)]);
    assert_eq!(l.stride, 32);
    assert!(!l.streamed);
    assert_eq!(l.position, Some((0, Encoding::Float3)));
    assert_eq!(l.normal, Some((12, Encoding::R10G10B10A2)));
    assert_eq!(l.color, Some((16, Encoding::Unorm8x4)));
    assert_eq!(l.uv, Some((20, Encoding::Half2)));
    assert_eq!(l.uv1, Some((24, Encoding::Float2)));
    assert_eq!(l.weights, Some((28, Encoding::Unorm8x4)));
    assert_eq!(l.indices, Some((32, Encoding::Uint8x4)));
}

#[test]
fn enhanced_declarations_know_more_formats() {
    let l = layout_of(&[(0, 4, 2), (4, 8, 10), (24, 0, 6)]);
    assert_eq!(l.position, Some((4, Encoding::Float4)));
    assert_eq!(l.normal, Some((8, Encoding::Half4)));
    assert_eq!(l.color, Some((0, Encoding::Float3)));
}

#[test]
fn enhanced_slots_without_a_type_or_with_an_unknown_one_are_skipped() {
    let l = layout_of(&[(0, 0, 6), (4, 12, 99), (28, 20, 0)]);
    assert_eq!(l.position, Some((0, Encoding::Float3)));
    assert!(l.normal.is_none() && l.uv.is_none());
}

#[test]
fn a_declaration_out_of_the_resource_is_an_error() {
    let m = Mem::new();
    assert!(matches!(gen9_layout(&m.res(159), SYS + 0x1000, 16), Err(ResourceError::OutOfBounds { .. })));
}

#[test]
fn legacy_declarations_stack_their_components() {
    let mut m = Mem::new();
    // position float3, weights unorm, indices ubyte4, normal float3, colour unorm, (5: float4), uv half2, uv1 float2
    let kinds = [6u64, 9, 8, 6, 9, 7, 1, 5];
    let types = kinds.iter().enumerate().fold(0u64, |acc, (k, t)| acc | t << (k * 4));
    let decl = legacy_decl(&mut m, 0xFF, types);
    let l = legacy_layout(&m.res(165), decl, 64).unwrap();
    assert_eq!(l.stride, 64);
    assert_eq!(l.position, Some((0, Encoding::Float3)));
    assert_eq!(l.weights, Some((12, Encoding::Unorm8x4)));
    assert_eq!(l.indices, Some((16, Encoding::Uint8x4)));
    assert_eq!(l.normal, Some((20, Encoding::Float3)));
    assert_eq!(l.color, Some((32, Encoding::Unorm8x4)));
    assert_eq!(l.uv, Some((52, Encoding::Half2)));
    assert_eq!(l.uv1, Some((56, Encoding::Float2)));
}

#[test]
fn legacy_components_without_an_encoding_still_take_room() {
    let mut m = Mem::new();
    // position float3, a single float (no encoding), snorm normal... at k = 3, uv half4 at k = 6 is not offered.
    let types = 6u64 | 2 << 4 | 10 << 12;
    let decl = legacy_decl(&mut m, 0b1011, types);
    let l = legacy_layout(&m.res(165), decl, 20).unwrap();
    assert_eq!(l.position, Some((0, Encoding::Float3)));
    assert!(l.weights.is_none(), "a single float has no encoding");
    assert_eq!(l.normal, Some((16, Encoding::Snorm8x4)), "after the float3 and the float");
}

#[test]
fn legacy_declarations_skip_components_that_are_not_set() {
    let mut m = Mem::new();
    let decl = legacy_decl(&mut m, 0b1, 6 | 0xF << 4);
    let l = legacy_layout(&m.res(165), decl, 12).unwrap();
    assert!(l.position.is_some());
    assert!(l.weights.is_none(), "type 15 of a component that is not set is not looked at");
}

#[test]
fn an_unknown_legacy_component_type_is_an_error() {
    let mut m = Mem::new();
    let decl = legacy_decl(&mut m, 0b1, 4);
    let err = legacy_layout(&m.res(165), decl, 12).unwrap_err();
    assert_eq!(err.to_string(), "unknown vertex component type 4");
}

#[test]
fn encodings_decode_their_values() {
    let f = |v: &[f32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    assert_eq!(Encoding::Float2.decode(&f(&[1.0, 2.0])), [1.0, 2.0, 0.0, 0.0]);
    assert_eq!(Encoding::Float3.decode(&f(&[1.0, 2.0, 3.0])), [1.0, 2.0, 3.0, 0.0]);
    assert_eq!(Encoding::Float4.decode(&f(&[1.0, 2.0, 3.0, 4.0])), [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(Encoding::Half4.decode(&[0, 0x3C, 0, 0x40, 0, 0xC0, 0, 0]), [1.0, 2.0, -2.0, 0.0]);
    assert_eq!(Encoding::Half2.decode(&[0, 0x3C, 0, 0x40]), [1.0, 2.0, 0.0, 0.0]);
    assert_eq!(Encoding::Unorm8x4.decode(&[0, 255, 51, 255]), [0.0, 1.0, 0.2, 1.0]);
    assert_eq!(Encoding::Uint8x4.decode(&[0, 1, 200, 255]), [0.0, 1.0, 200.0, 255.0]);
    assert_eq!(Encoding::Snorm8x4.decode(&[127, 0x80, 0, 0x81]), [1.0, -1.0, 0.0, -1.0]);
}

#[test]
fn ten_bit_normals_decode_both_ways() {
    let v: u32 = 0x3FF | 512 << 20 | 3 << 30;
    let signed = Encoding::R10G10B10A2.decode(&v.to_le_bytes());
    assert_eq!(signed[0], -1.0 / 511.0);
    assert_eq!(signed[1], 0.0);
    assert_eq!(signed[2], -1.0);
    assert_eq!(signed[3], 1.0);
    let unsigned = Encoding::R10G10B10A2Unorm.decode(&0xFFFF_FFFFu32.to_le_bytes());
    assert_eq!(unsigned, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(Encoding::R10G10B10A2Unorm.decode(&[0; 4]), [-1.0, -1.0, -1.0, 0.0]);
}

#[test]
fn every_encoding_has_a_size() {
    let sizes = [
        (Encoding::Float2, 8),
        (Encoding::Float3, 12),
        (Encoding::Float4, 16),
        (Encoding::Half2, 4),
        (Encoding::Half4, 8),
        (Encoding::Unorm8x4, 4),
        (Encoding::Snorm8x4, 4),
        (Encoding::Uint8x4, 4),
        (Encoding::R10G10B10A2, 4),
        (Encoding::R10G10B10A2Unorm, 4),
    ];
    for (e, n) in sizes {
        assert_eq!(e.size(), n, "{e:?}");
    }
}

const ENHANCED_SLOTS: [(usize, u32, u8); 5] = [(0, 0, 6), (4, 12, 24), (24, 16, 28), (28, 20, 34), (29, 24, 16)];

/// Three vertices of 32 bytes: position, packed normal, colour, half uv, float uv1.
fn enhanced_vertices() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..3u32 {
        for x in [i as f32, i as f32 + 1.0, i as f32 + 2.0] {
            out.extend(x.to_le_bytes());
        }
        out.extend(511u32.to_le_bytes());
        out.extend([255, 0, 0, 255]);
        out.extend([0x00, 0x3C, 0x00, 0x40]);
        out.extend(0.5f32.to_le_bytes());
        out.extend(0.25f32.to_le_bytes());
    }
    out
}

fn sga_geometry(
    m: &mut Mem,
    layout_gen: Gen,
    (count, stride): (u32, u16),
    vdata: &[u8],
    decl: u64,
    indices: &[u32],
    index_size: u16,
) -> u64 {
    let vb = m.alloc(0x40);
    m.u32(vb + 8, count);
    m.u16(vb + 0x0C, stride);
    let data = m.bytes(vdata);
    m.ptr(vb + 0x18, data);
    m.ptr(vb + 0x38, decl);
    let ib = m.alloc(0x20);
    m.u32(ib + 8, indices.len() as u32);
    m.u16(ib + 0x0C, index_size);
    let idata: Vec<u8> = if index_size == 4 {
        indices.iter().flat_map(|i| i.to_le_bytes()).collect()
    } else {
        indices.iter().flat_map(|i| (*i as u16).to_le_bytes()).collect()
    };
    let idata = m.bytes(&idata);
    m.ptr(ib + 0x18, idata);
    let g = m.alloc(0x80);
    if layout_gen == Gen::Rdr2 {
        m.ptr(g + 0x08, vb);
        m.ptr(g + 0x10, ib);
    } else {
        m.ptr(g + 0x18, vb);
        m.ptr(g + 0x38, ib);
    }
    g
}

fn enhanced_geometry(m: &mut Mem, indices: &[u32]) -> u64 {
    let decl = gen9_decl(m, &ENHANCED_SLOTS);
    sga_geometry(m, Gen::Enhanced, (3, 32), &enhanced_vertices(), decl, indices, 2)
}

fn legacy_geometry(m: &mut Mem, count: u32, stride: u16, vdata: &[u8], decl: u64, indices: &[u16]) -> u64 {
    let vb = m.alloc(0x40);
    m.u16(vb + 0x08, stride);
    let data = m.bytes(vdata);
    m.ptr(vb + 0x10, data);
    m.u32(vb + 0x18, count);
    m.ptr(vb + 0x30, decl);
    let ib = m.alloc(0x20);
    m.u32(ib + 8, indices.len() as u32);
    let idata = m.bytes(&indices.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>());
    m.ptr(ib + 0x10, idata);
    let g = m.alloc(0x80);
    m.ptr(g + 0x18, vb);
    m.ptr(g + 0x38, ib);
    g
}

/// A model with `geoms`, a shader mapping and its skeleton binding at `binding_at`.
fn model(m: &mut Mem, geoms: &[u64], mapping: Option<&[u16]>, binding_at: u64, binding: u32) -> u64 {
    let model = m.alloc(0x40);
    let list = m.alloc(geoms.len() * 8);
    for (i, g) in geoms.iter().enumerate() {
        m.ptr(list + i as u64 * 8, *g);
    }
    m.ptr(model + 0x08, list);
    m.u16(model + 0x10, geoms.len() as u16);
    if let Some(mapping) = mapping {
        let at = m.bytes(&mapping.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
        m.ptr(model + 0x20, at);
    }
    m.u32(model + binding_at, binding);
    model
}

/// A list header (pointer, count) of `models`.
fn model_list(m: &mut Mem, models: &[u64]) -> u64 {
    let array = m.alloc(models.len() * 8);
    for (i, md) in models.iter().enumerate() {
        m.ptr(array + i as u64 * 8, *md);
    }
    let header = m.alloc(16);
    m.ptr(header, array);
    m.u16(header + 8, models.len() as u16);
    header
}

/// A drawable structure whose detail level `lod` lists `models`.
fn drawable_at(m: &mut Mem, at: Option<u64>, lod: u64, models: &[u64]) -> u64 {
    let d = at.unwrap_or_else(|| m.alloc(0x80));
    let list = model_list(m, models);
    m.ptr(d + 0x50 + lod * 8, list);
    d
}

fn enhanced_drawable(indices: &[u32]) -> Resource {
    let mut m = Mem::new();
    let g = enhanced_geometry(&mut m, indices);
    let md = model(&mut m, &[g], Some(&[3]), 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    m.res(VERSION_YDR_ENHANCED)
}

#[test]
fn an_enhanced_drawable_gives_its_vertices_and_indices() {
    let res = enhanced_drawable(&[0, 1, 2]);
    let d = parse_ydr(&res).unwrap();
    assert_eq!(d.meshes.len(), 1);
    let mesh = &d.meshes[0];
    assert_eq!(mesh.positions, [[0.0, 1.0, 2.0], [1.0, 2.0, 3.0], [2.0, 3.0, 4.0]]);
    assert_eq!(mesh.normals, [[1.0, 0.0, 0.0]; 3]);
    assert_eq!(mesh.colors, [[1.0, 0.0, 0.0, 1.0]; 3]);
    assert_eq!(mesh.uvs, [[1.0, 2.0]; 3]);
    assert_eq!(mesh.uvs1, [[0.5, 0.25]; 3]);
    assert_eq!(mesh.indices, [0, 1, 2]);
    assert_eq!(mesh.shader, 3, "from the model's shader mapping");
    assert!(mesh.bone_indices.is_empty() && mesh.bone_weights.is_empty());
    assert_eq!((d.triangle_count(), d.vertex_count()), (1, 3));
    assert!(d.skeleton.is_none() && d.textures.is_none() && d.shaders.is_empty());
}

#[test]
fn the_bounds_of_a_drawable_are_read() {
    let mut m = Mem::new();
    m.f32s(SYS + 0x30, &[-1.0, -2.0, -3.0]);
    m.f32s(SYS + 0x40, &[4.0, 5.0, 6.0]);
    let d = parse_ydr(&m.res(VERSION_YDR_ENHANCED)).unwrap();
    assert_eq!((d.bounds_min, d.bounds_max), ([-1.0, -2.0, -3.0], [4.0, 5.0, 6.0]));
    assert!(d.meshes.is_empty());
}

#[test]
fn a_model_without_a_shader_mapping_uses_shader_zero() {
    let mut m = Mem::new();
    let g = enhanced_geometry(&mut m, &[0, 1, 2]);
    let md = model(&mut m, &[g, g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    let d = parse_ydr(&m.res(VERSION_YDR_ENHANCED)).unwrap();
    assert_eq!(d.meshes.iter().map(|m| m.shader).collect::<Vec<_>>(), [0, 0]);
}

#[test]
fn indices_of_four_bytes_are_read() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &ENHANCED_SLOTS);
    let g = sga_geometry(&mut m, Gen::Enhanced, (3, 32), &enhanced_vertices(), decl, &[2, 1, 0], 4);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    let d = parse_ydr(&m.res(VERSION_YDR_ENHANCED)).unwrap();
    assert_eq!(d.meshes[0].indices, [2, 1, 0]);
}

#[test]
fn leftover_indices_that_make_no_triangle_are_dropped() {
    let d = parse_ydr(&enhanced_drawable(&[0, 1, 2, 2, 1])).unwrap();
    assert_eq!(d.meshes[0].indices, [0, 1, 2]);
}

#[test]
fn an_index_beyond_the_vertices_is_an_error() {
    let err = parse_ydr(&enhanced_drawable(&[0, 1, 3])).unwrap_err();
    assert_eq!(err.to_string(), "index outside the vertex buffer");
}

#[test]
fn an_implausible_vertex_buffer_is_an_error() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &ENHANCED_SLOTS);
    let g = sga_geometry(&mut m, Gen::Enhanced, (3, 0), &enhanced_vertices(), decl, &[0, 1, 2], 2);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    assert_eq!(parse_ydr(&m.res(159)).unwrap_err().to_string(), "implausible vertex buffer");
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &ENHANCED_SLOTS);
    let g = sga_geometry(&mut m, Gen::Enhanced, ((1 << 22) + 1, 32), &enhanced_vertices(), decl, &[0, 1, 2], 2);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    assert_eq!(parse_ydr(&m.res(159)).unwrap_err().to_string(), "implausible vertex buffer");
}

#[test]
fn vertices_without_a_position_are_an_error() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &[(4, 12, 24)]);
    let g = sga_geometry(&mut m, Gen::Enhanced, (3, 32), &enhanced_vertices(), decl, &[0, 1, 2], 2);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    assert_eq!(parse_ydr(&m.res(159)).unwrap_err().to_string(), "vertices without positions");
}

#[test]
fn a_position_that_does_not_fit_in_the_stride_is_an_error() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &[(0, 0, 6)]);
    let g = sga_geometry(&mut m, Gen::Enhanced, (3, 8), &enhanced_vertices(), decl, &[0, 1, 2], 2);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    assert_eq!(parse_ydr(&m.res(159)).unwrap_err().to_string(), "position outside vertex");
}

#[test]
fn attributes_that_do_not_fit_in_the_stride_are_left_out() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &[(0, 0, 6), (28, 30, 34)]);
    let g = sga_geometry(&mut m, Gen::Enhanced, (3, 32), &enhanced_vertices(), decl, &[0, 1, 2], 2);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    let d = parse_ydr(&m.res(159)).unwrap();
    assert_eq!(d.meshes[0].positions.len(), 3);
    assert!(d.meshes[0].uvs.is_empty());
}

#[test]
fn vertex_data_beyond_the_resource_is_an_error() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &ENHANCED_SLOTS);
    let g = sga_geometry(&mut m, Gen::Enhanced, (1000, 32), &enhanced_vertices(), decl, &[0, 1, 2], 2);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    assert!(matches!(parse_ydr(&m.res(159)), Err(ResourceError::OutOfBounds { .. })));
}

#[test]
fn the_first_detail_level_with_meshes_is_read() {
    let mut m = Mem::new();
    let g = enhanced_geometry(&mut m, &[0, 1, 2]);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[]);
    drawable_at(&mut m, Some(SYS), 1, &[0, md]);
    drawable_at(&mut m, Some(SYS), 2, &[md, md]);
    let d = parse_ydr(&m.res(159)).unwrap();
    assert_eq!(d.meshes.len(), 1, "lod 0 is empty, lod 1 wins, a null model is skipped");
}

#[test]
fn unsupported_resource_versions_are_refused() {
    let m = Mem::new();
    assert_eq!(parse_ydr(&m.res(5)).unwrap_err().to_string(), "unsupported drawable resource version 5");
    let m = Mem::new();
    assert!(parse_yft(&m.res(7)).is_err());
    let m = Mem::new();
    assert!(parse_ydd(&m.res(7)).is_err());
}

#[test]
fn the_old_enhanced_version_is_read_like_enhanced() {
    let mut res = enhanced_drawable(&[0, 1, 2]);
    res.version = VERSION_YDR_ENHANCED_OLD;
    assert_eq!(parse_ydr(&res).unwrap().meshes.len(), 1);
}

fn legacy_skinned(table: Option<(&[u16], u16)>) -> Resource {
    let mut m = Mem::new();
    let types = 6u64 | 9 << 4 | 8 << 8;
    let decl = legacy_decl(&mut m, 0b111, types);
    let mut data = Vec::new();
    for (i, indices) in [[0u8, 1, 2, 3], [2, 2, 2, 2]].iter().enumerate() {
        for x in [i as f32, 0.0, 0.0] {
            data.extend(x.to_le_bytes());
        }
        data.extend([255, 0, 0, 0]);
        data.extend(*indices);
    }
    let g = legacy_geometry(&mut m, 2, 20, &data, decl, &[0, 1, 0]);
    if let Some((ids, count)) = table {
        let at = m.bytes(&ids.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
        m.ptr(g + 0x68, at);
        m.u16(g + 0x72, count);
    }
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    m.res(VERSION_YDR_LEGACY)
}

#[test]
fn legacy_blend_indices_go_through_the_bone_table() {
    let d = parse_ydr(&legacy_skinned(Some((&[10, 11, 12], 3)))).unwrap();
    let mesh = &d.meshes[0];
    assert_eq!(mesh.positions, [[0.0; 3], [1.0, 0.0, 0.0]]);
    assert_eq!(mesh.bone_indices, [[10, 11, 12, 0], [12, 12, 12, 12]], "an index beyond the table gives 0");
    assert_eq!(mesh.bone_weights, [[1.0, 0.0, 0.0, 0.0]; 2]);
    assert_eq!(mesh.indices, [0, 1, 0]);
}

#[test]
fn legacy_blend_indices_stay_without_a_bone_table() {
    let d = parse_ydr(&legacy_skinned(None)).unwrap();
    assert_eq!(d.meshes[0].bone_indices, [[0, 1, 2, 3], [2, 2, 2, 2]]);
    let d = parse_ydr(&legacy_skinned(Some((&[10, 11, 12], 0)))).unwrap();
    assert_eq!(d.meshes[0].bone_indices[0], [0, 1, 2, 3], "a table of no entries");
    let d = parse_ydr(&legacy_skinned(Some((&[10, 11, 12], 2000)))).unwrap();
    assert_eq!(d.meshes[0].bone_indices[0], [0, 1, 2, 3], "a table too large to be real");
}

#[test]
fn a_legacy_drawable_gives_its_attributes() {
    let mut m = Mem::new();
    let types = 6u64 | 6 << 12 | 9 << 16 | 1 << 24 | 5 << 28;
    let decl = legacy_decl(&mut m, 0b1111_1001, types);
    let mut data = Vec::new();
    for i in 0..3 {
        for x in [i as f32, 1.0, 2.0, 0.0, 0.0, 1.0] {
            data.extend(x.to_le_bytes());
        }
        data.extend([0, 255, 0, 255]);
        data.extend([0x00, 0x3C, 0x00, 0x40]);
        data.extend(0.5f32.to_le_bytes());
        data.extend(0.75f32.to_le_bytes());
    }
    let g = legacy_geometry(&mut m, 3, 40, &data, decl, &[0, 1, 2]);
    let md = model(&mut m, &[g], None, 0x28, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    let d = parse_ydr(&m.res(VERSION_YDR_LEGACY)).unwrap();
    let mesh = &d.meshes[0];
    assert_eq!(mesh.positions[2], [2.0, 1.0, 2.0]);
    assert_eq!(mesh.normals, [[0.0, 0.0, 1.0]; 3]);
    assert_eq!(mesh.colors, [[0.0, 1.0, 0.0, 1.0]; 3]);
    assert_eq!(mesh.uvs, [[1.0, 2.0]; 3]);
    assert_eq!(mesh.uvs1, [[0.5, 0.75]; 3]);
}

/// A skeleton structure with bones `(name, tag, parent, translation)` in the GTA V or the RDR2 layout.
fn skeleton(m: &mut Mem, rdr2: bool, bones: &[(&str, u16, i16, [f32; 3])]) -> u64 {
    let sk = m.alloc(0x80);
    let array = m.alloc(bones.len() * 0x50);
    let (list, name, tag, parent) = if rdr2 { (0x28, 0x30, 0x4A, 0x46) } else { (0x20, 0x38, 0x44, 0x32) };
    for (i, (n, t, p, tr)) in bones.iter().enumerate() {
        let b = array + i as u64 * 0x50;
        m.f32s(b, &[0.0, 0.0, 0.0, 1.0]);
        m.f32s(b + 0x10, tr);
        m.f32s(b + 0x20, &[1.0, 1.0, 1.0]);
        if !n.is_empty() {
            let s = m.string(n);
            m.ptr(b + name, s);
        }
        m.u16(b + tag, *t);
        m.u16(b + parent, *p as u16);
    }
    m.ptr(sk + list, array);
    m.u16(sk + 0x5E, bones.len() as u16);
    sk
}

const BONES: [(&str, u16, i16, [f32; 3]); 3] =
    [("root", 100, -1, [0.0; 3]), ("arm", 200, 0, [10.0, 0.0, 0.0]), ("hand", 300, 1, [0.0, 5.0, 0.0])];

#[test]
fn a_gta5_skeleton_gives_its_bones() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    let s = read_skeleton_gen(&m.res(159), sk, Gen::Enhanced).unwrap();
    assert_eq!(s.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["root", "arm", "hand"]);
    assert_eq!(s.bones.iter().map(|b| (b.tag, b.parent)).collect::<Vec<_>>(), [(100, -1), (200, 0), (300, 1)]);
    assert_eq!(s.bones[1].translation, [10.0, 0.0, 0.0]);
    assert_eq!(s.bones[1].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(s.bones[1].scale, [1.0; 3]);
}

#[test]
fn an_rdr2_skeleton_keeps_its_fields_elsewhere() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, true, &BONES);
    let res = m.res(159);
    let s = read_skeleton_gen(&res, sk, Gen::Rdr2).unwrap();
    assert_eq!(
        s.bones.iter().map(|b| (b.name.as_str(), b.tag, b.parent)).collect::<Vec<_>>(),
        [("root", 100, -1), ("arm", 200, 0), ("hand", 300, 1)]
    );
    assert!(read_skeleton_gen(&res, sk, Gen::Enhanced).is_err(), "GTA V's layout finds no bones list there");
}

#[test]
fn a_skeleton_without_bones_or_with_too_many_is_invalid() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    m.u16(sk + 0x5E, 2049);
    assert_eq!(read_skeleton_gen(&m.res(159), sk, Gen::Enhanced).unwrap_err().to_string(), "invalid skeleton");
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    m.ptr(sk + 0x20, 0);
    assert_eq!(read_skeleton_gen(&m.res(159), sk, Gen::Enhanced).unwrap_err().to_string(), "invalid skeleton");
}

#[test]
fn a_bone_without_a_name_has_an_empty_one() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &[("", 5, -1, [0.0; 3])]);
    let s = read_skeleton_gen(&m.res(159), sk, Gen::Enhanced).unwrap();
    assert_eq!(s.bones[0].name, "");
}

#[test]
fn world_transforms_apply_the_parents() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    let s = read_skeleton_gen(&m.res(159), sk, Gen::Enhanced).unwrap();
    let world = s.world_transforms();
    assert_eq!(world.len(), 3);
    assert_eq!(math::transform_point(&world[2], [0.0; 3]), [10.0, 5.0, 0.0]);
    assert_eq!(math::transform_point(&world[1], [1.0, 0.0, 0.0]), [11.0, 0.0, 0.0]);
}

#[test]
fn a_parent_that_comes_later_is_ignored() {
    let s = Skeleton {
        bones: vec![
            Bone {
                parent: 1,
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                ..Default::default()
            },
            Bone {
                parent: -1,
                translation: [0.0, 2.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                ..Default::default()
            },
        ],
    };
    let world = s.world_transforms();
    assert_eq!(math::transform_point(&world[0], [0.0; 3]), [1.0, 0.0, 0.0]);
}

#[test]
fn bones_are_found_by_tag_name_and_hash() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    let s = read_skeleton_gen(&m.res(159), sk, Gen::Enhanced).unwrap();
    assert_eq!(s.find_tag(300), Some(2));
    assert_eq!(s.find_tag(1), None);
    assert_eq!(s.find_name("ARM"), Some(1));
    assert_eq!(s.find_name("leg"), None);
    assert_eq!(s.find_hash(rage_core::hash::joaat("hand")), Some(2));
    assert_eq!(s.find_hash(1), None);
}

#[test]
fn a_drawable_with_a_skeleton_reads_it() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    m.ptr(SYS + 0x18, sk);
    assert_eq!(parse_ydr(&m.res(159)).unwrap().skeleton.unwrap().bones.len(), 3);
}

#[test]
fn a_damaged_skeleton_is_left_out_of_the_drawable() {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    m.u16(sk + 0x5E, 3000);
    m.ptr(SYS + 0x18, sk);
    assert!(parse_ydr(&m.res(159)).unwrap().skeleton.is_none());
}

fn rigid_drawable(binding: u32) -> Drawable {
    let mut m = Mem::new();
    let sk = skeleton(&mut m, false, &BONES);
    m.ptr(SYS + 0x18, sk);
    let g = enhanced_geometry(&mut m, &[0, 1, 2]);
    let md = model(&mut m, &[g], None, 0x28, binding);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    parse_ydr(&m.res(159)).unwrap()
}

#[test]
fn a_model_attached_to_a_bone_moves_with_it() {
    let d = rigid_drawable(1 << 24);
    let mesh = &d.meshes[0];
    assert_eq!(mesh.positions[0], [10.0, 1.0, 2.0], "moved by the bone's place");
    assert_eq!(mesh.normals[0], [1.0, 0.0, 0.0]);
    assert_eq!(mesh.bone_indices, [[1, 0, 0, 0]; 3]);
    assert_eq!(mesh.bone_weights, [[1.0, 0.0, 0.0, 0.0]; 3]);
}

#[test]
fn a_model_on_a_bone_that_does_not_exist_stays_put() {
    let d = rigid_drawable(9 << 24);
    assert_eq!(d.meshes[0].positions[0], [0.0, 1.0, 2.0]);
    assert!(d.meshes[0].bone_indices.is_empty());
}

#[test]
fn a_skinned_model_is_not_attached_to_one_bone() {
    let d = rigid_drawable(1 << 24 | 1 << 8);
    assert_eq!(d.meshes[0].positions[0], [0.0, 1.0, 2.0]);
    assert!(d.meshes[0].bone_indices.is_empty());
}

fn texture(m: &mut Mem, name: &str) -> u64 {
    let t = m.alloc(0x40);
    let s = m.string(name);
    m.ptr(t + 0x28, s);
    t
}

/// An Enhanced shader: textures with the slots `(hash, slot)` of its parameter block.
fn sga_shader(m: &mut Mem, name_hash: u32, textures: &[u64], num: u8, params: &[(u32, u32)]) -> u64 {
    let s = m.alloc(0x30);
    m.u32(s, name_hash);
    let ptrs = m.alloc(textures.len() * 8);
    for (i, t) in textures.iter().enumerate() {
        m.ptr(ptrs + i as u64 * 8, *t);
    }
    m.ptr(s + 0x10, ptrs);
    let infos = m.alloc(8 + params.len() * 8);
    m.u8(infos + 1, num);
    m.u8(infos + 4, params.len() as u8);
    for (k, (hash, slot)) in params.iter().enumerate() {
        m.u32(infos + 8 + k as u64 * 8, *hash);
        m.u32(infos + 12 + k as u64 * 8, *slot);
    }
    m.ptr(s + 0x20, infos);
    s
}

fn group(m: &mut Mem, shaders: &[u64]) -> u64 {
    let g = m.alloc(0x40);
    let list = m.alloc(shaders.len() * 8);
    for (i, s) in shaders.iter().enumerate() {
        m.ptr(list + i as u64 * 8, *s);
    }
    m.ptr(g + 0x10, list);
    m.u16(g + 0x18, shaders.len() as u16);
    g
}

#[test]
fn enhanced_shaders_name_their_textures_by_slot() {
    let mut m = Mem::new();
    let (a, b) = (texture(&mut m, "diffuse_a"), texture(&mut m, "normal_b"));
    let s = sga_shader(&mut m, 0x1234, &[a, b], 2, &[(param::DIFFUSE_TEX, 0), (param::BUMP_TEX, 4)]);
    let g = group(&mut m, &[s]);
    m.ptr(SYS + 0x10, g);
    let d = parse_ydr(&m.res(159)).unwrap();
    assert_eq!(d.shaders.len(), 1);
    let s = &d.shaders[0];
    assert_eq!(s.name_hash, 0x1234);
    assert_eq!(s.textures, ["diffuse_a", "normal_b"]);
    assert_eq!(s.samplers, [(param::DIFFUSE_TEX, "diffuse_a".to_string()), (param::BUMP_TEX, "normal_b".to_string())]);
    assert_eq!(s.sampler(param::BUMP_TEX), Some("normal_b"));
    assert_eq!(s.base_texture(), Some("diffuse_a"));
}

#[test]
fn parameters_that_are_not_textures_or_have_no_slot_are_skipped() {
    let mut m = Mem::new();
    let a = texture(&mut m, "diffuse_a");
    let s = sga_shader(&mut m, 1, &[a, 0], 2, &[(7, 1), (8, 12), (param::DIFFUSE_TEX, 0), (9, 4)]);
    let g = group(&mut m, &[s]);
    m.ptr(SYS + 0x10, g);
    let d = parse_ydr(&m.res(159)).unwrap();
    assert_eq!(d.shaders[0].samplers, [(param::DIFFUSE_TEX, "diffuse_a".to_string())], "a null texture is skipped");
}

#[test]
fn a_shader_without_a_parameter_block_has_no_textures() {
    let mut m = Mem::new();
    let a = texture(&mut m, "diffuse_a");
    let s = sga_shader(&mut m, 1, &[a], 1, &[(param::DIFFUSE_TEX, 0)]);
    m.ptr(s + 0x20, 0);
    let g = group(&mut m, &[s]);
    m.ptr(SYS + 0x10, g);
    assert!(parse_ydr(&m.res(159)).unwrap().shaders[0].textures.is_empty());
}

#[test]
fn texture_names_must_be_plain_and_short() {
    let mut m = Mem::new();
    let long = "x".repeat(128);
    let names = ["", "has space", "ünïcode", long.as_str(), "fine_name", "fine_name"];
    let textures: Vec<u64> = names.iter().map(|n| texture(&mut m, n)).collect();
    let params: Vec<(u32, u32)> = (0..6).map(|i| (100 + i, i * 4)).collect();
    let s = sga_shader(&mut m, 1, &textures, 6, &params);
    let g = group(&mut m, &[s]);
    m.ptr(SYS + 0x10, g);
    let shader = parse_ydr(&m.res(159)).unwrap().shaders.remove(0);
    assert_eq!(shader.textures, ["fine_name"], "a name used twice is listed once");
    assert_eq!(shader.samplers, [(104, "fine_name".to_string()), (105, "fine_name".to_string())]);
}

fn legacy_shader(m: &mut Mem, name_hash: u32, params: &[(u8, Option<u64>, u32)]) -> u64 {
    let s = m.alloc(0x20);
    let data: u64 = params.iter().map(|p| p.0 as u64).sum::<u64>() * 16;
    let block = m.alloc(params.len() * 16 + data as usize + params.len() * 4);
    for (i, (t, tex, hash)) in params.iter().enumerate() {
        m.u8(block + i as u64 * 16, *t);
        if let Some(tex) = tex {
            m.ptr(block + i as u64 * 16 + 8, *tex);
        }
        m.u32(block + params.len() as u64 * 16 + data + i as u64 * 4, *hash);
    }
    m.ptr(s, block);
    m.u32(s + 8, name_hash);
    m.u8(s + 0x10, params.len() as u8);
    s
}

#[test]
fn legacy_shaders_keep_their_textures_among_other_parameters() {
    let mut m = Mem::new();
    let (a, b) = (texture(&mut m, "diffuse_a"), texture(&mut m, "spec_b"));
    let s = legacy_shader(
        &mut m,
        0x77,
        &[(0, Some(a), param::DIFFUSE_SAMPLER), (1, None, 5), (0, Some(b), param::SPEC_SAMPLER), (0, None, 6)],
    );
    let g = group(&mut m, &[s]);
    m.ptr(SYS + 0x10, g);
    let d = parse_ydr(&m.res(VERSION_YDR_LEGACY)).unwrap();
    let shader = &d.shaders[0];
    assert_eq!(shader.name_hash, 0x77);
    assert_eq!(
        shader.samplers,
        [(param::DIFFUSE_SAMPLER, "diffuse_a".to_string()), (param::SPEC_SAMPLER, "spec_b".to_string())]
    );
    assert_eq!(shader.base_texture(), Some("diffuse_a"));
}

#[test]
fn at_most_512_shaders_are_read() {
    let mut m = Mem::new();
    let s = sga_shader(&mut m, 1, &[], 0, &[]);
    let g = group(&mut m, &vec![s; 600]);
    m.ptr(SYS + 0x10, g);
    assert_eq!(parse_ydr(&m.res(159)).unwrap().shaders.len(), 512);
}

#[test]
fn damaged_shaders_leave_the_drawable_without_any() {
    let mut m = Mem::new();
    let s = sga_shader(&mut m, 1, &[], 0, &[]);
    let g = group(&mut m, &[s, SYS + 0x10_0000]);
    m.ptr(SYS + 0x10, g);
    let res = m.res(159);
    assert!(read_shaders(&res, g, Gen::Enhanced).is_err());
    assert!(parse_ydr(&res).unwrap().shaders.is_empty());
}

#[test]
fn an_embedded_texture_dictionary_that_cannot_be_read_is_left_out() {
    let mut m = Mem::new();
    let g = group(&mut m, &[]);
    m.ptr(g + 8, SYS + 0x10_0000);
    m.ptr(SYS + 0x10, g);
    let res = m.res(159);
    assert!(parse_ydr(&res).unwrap().textures.is_none());
    assert!(parse_ydr_rdr2(&res).unwrap().textures.is_none());
}

/// A group whose texture dictionary is at `txd`.
fn group_with_dict(m: &mut Mem, txd: u64) -> u64 {
    let g = group(m, &[]);
    m.ptr(g + 8, txd);
    g
}

#[test]
fn a_drawable_carries_one_texture_dictionary() {
    let mut m = Mem::new();
    let g = group_with_dict(&mut m, SYS + 0x180);
    m.ptr(SYS + 0x10, g);
    let (enhanced, dicts) = embedded_texture_dicts(&m.res(159), "ydr").unwrap();
    assert!(enhanced);
    assert_eq!(dicts, [SYS + 0x180]);
}

#[test]
fn a_drawable_without_a_group_or_a_dictionary_carries_none() {
    let m = Mem::new();
    assert_eq!(embedded_texture_dicts(&m.res(165), "YDR").unwrap(), (false, Vec::new()));
    let mut m = Mem::new();
    let g = group_with_dict(&mut m, 0);
    m.ptr(SYS + 0x10, g);
    assert!(embedded_texture_dicts(&m.res(165), "ydr").unwrap().1.is_empty());
}

#[test]
fn a_fragment_and_its_parts_carry_each_dictionary_once() {
    let mut m = Mem::new();
    let body = m.alloc(0x80);
    let g = group_with_dict(&mut m, SYS + 0x180);
    m.ptr(body + 0x10, g);
    let part_a = m.alloc(0x80);
    let g2 = group_with_dict(&mut m, SYS + 0x180);
    m.ptr(part_a + 0x10, g2);
    let part_b = m.alloc(0x80);
    let g3 = group_with_dict(&mut m, SYS + 0x1C0);
    m.ptr(part_b + 0x10, g3);
    let lod = m.alloc(0x130);
    let list = m.alloc(24);
    let children: Vec<u64> = [part_a, part_b, 0]
        .iter()
        .map(|p| {
            let c = m.alloc(0xB0);
            m.ptr(c + 0xA0, *p);
            c
        })
        .collect();
    m.ptr(list, children[0]);
    m.ptr(list + 8, children[1]);
    m.ptr(list + 16, children[2]);
    m.ptr(lod + 0xD0, list);
    m.u8(lod + 0x11E, 3);
    let group = m.alloc(0x40);
    m.ptr(group + 0x10, lod);
    m.ptr(SYS + 0x30, body);
    m.ptr(SYS + 0xF0, group);
    let (enhanced, dicts) = embedded_texture_dicts(&m.res(VERSION_YFT_ENHANCED), "yft").unwrap();
    assert!(enhanced);
    assert_eq!(dicts, [SYS + 0x180, SYS + 0x1C0]);
}

#[test]
fn a_drawable_dictionary_carries_the_dictionaries_of_its_drawables() {
    let mut m = Mem::new();
    let a = m.alloc(0x80);
    let g = group_with_dict(&mut m, SYS + 0x180);
    m.ptr(a + 0x10, g);
    let b = m.alloc(0x80);
    let list = m.alloc(24);
    m.ptr(list, a);
    m.ptr(list + 8, b);
    m.ptr(SYS + 0x30, list);
    m.u16(SYS + 0x38, 3);
    let (_, dicts) = embedded_texture_dicts(&m.res(VERSION_YDR_LEGACY), "ydd").unwrap();
    assert_eq!(dicts, [SYS + 0x180]);
}

#[test]
fn texture_dictionaries_of_other_files_are_refused() {
    let m = Mem::new();
    assert_eq!(embedded_texture_dicts(&m.res(159), "ytd").unwrap_err().to_string(), "ytd files have no drawables");
    let m = Mem::new();
    assert_eq!(
        embedded_texture_dicts(&m.res(3), "ydr").unwrap_err().to_string(),
        "unsupported drawable resource version 3"
    );
}

/// A drawable of one triangle; with `enhanced` the Enhanced layout, else the Legacy one.
fn triangle_drawable(m: &mut Mem) -> u64 {
    let g = enhanced_geometry(m, &[0, 1, 2]);
    let md = model(m, &[g], None, 0x28, 0);
    drawable_at(m, None, 0, &[md])
}

fn fragment_with_child(child_tag: u16, child_mesh: bool, repeated: bool) -> Resource {
    let mut m = Mem::new();
    let body = triangle_drawable(&mut m);
    let name = m.string("pack:/vehicle_x");
    m.ptr(SYS + 0x30, body);
    m.ptr(SYS + 0x58, name);
    let part = if child_mesh { triangle_drawable(&mut m) } else { m.alloc(0x80) };
    let lod = m.alloc(0x130);
    let list = m.alloc(16);
    let mut kids = Vec::new();
    for _ in 0..if repeated { 2 } else { 1 } {
        let c = m.alloc(0xB0);
        m.u16(c + 0x12, child_tag);
        m.ptr(c + 0xA0, part);
        kids.push(c);
    }
    for (i, c) in kids.iter().enumerate() {
        m.ptr(list + i as u64 * 8, *c);
    }
    m.ptr(lod + 0xD0, list);
    m.u8(lod + 0x11E, kids.len() as u8);
    let group = m.alloc(0x40);
    m.ptr(group + 0x18, lod);
    m.ptr(SYS + 0xF0, group);
    m.res(VERSION_YFT_ENHANCED)
}

#[test]
fn a_fragment_has_a_name_and_children_with_meshes() {
    let f = parse_fragment(&fragment_with_child(77, true, false)).unwrap();
    assert_eq!(f.drawable.name, "vehicle_x", "the pack prefix is dropped");
    assert_eq!(f.drawable.meshes.len(), 1);
    assert_eq!(f.children.len(), 1);
    assert_eq!(f.children[0].bone_tag, 77);
    assert_eq!(f.children[0].drawable.meshes.len(), 1);
    assert!(f.cloths.is_empty());
    assert_eq!(parse_yft(&fragment_with_child(77, true, false)).unwrap().name, "vehicle_x");
}

#[test]
fn a_child_listed_twice_is_read_once_and_one_without_meshes_is_left_out() {
    assert_eq!(parse_fragment(&fragment_with_child(1, true, true)).unwrap().children.len(), 1);
    assert!(parse_fragment(&fragment_with_child(1, false, false)).unwrap().children.is_empty());
}

#[test]
fn a_fragment_needs_a_drawable() {
    let m = Mem::new();
    assert_eq!(parse_fragment(&m.res(VERSION_YFT_ENHANCED)).unwrap_err().to_string(), "fragment without drawable");
}

#[test]
fn a_fragment_without_a_physics_group_has_no_children() {
    let mut m = Mem::new();
    let body = triangle_drawable(&mut m);
    m.ptr(SYS + 0x30, body);
    let f = parse_fragment(&m.res(VERSION_YFT_ENHANCED)).unwrap();
    assert!(f.children.is_empty());
    assert_eq!(f.drawable.name, "");
}

#[test]
fn a_drawable_dictionary_pairs_hashes_with_drawables() {
    let mut m = Mem::new();
    let a = triangle_drawable(&mut m);
    let b = triangle_drawable(&mut m);
    let hashes = m.bytes(&[1, 0, 0, 0, 2, 0, 0, 0]);
    let list = m.alloc(16);
    m.ptr(list, a);
    m.ptr(list + 8, b);
    m.ptr(SYS + 0x20, hashes);
    m.ptr(SYS + 0x30, list);
    m.u16(SYS + 0x38, 2);
    let out = parse_ydd(&m.res(VERSION_YDR_ENHANCED)).unwrap();
    assert_eq!(out.iter().map(|(h, d)| (*h, d.meshes.len())).collect::<Vec<_>>(), [(1, 1), (2, 1)]);
}

#[test]
fn a_drawable_dictionary_without_hashes_gives_zero() {
    let mut m = Mem::new();
    let a = triangle_drawable(&mut m);
    let list = m.alloc(8);
    m.ptr(list, a);
    m.ptr(SYS + 0x30, list);
    m.u16(SYS + 0x38, 1);
    assert_eq!(parse_ydd(&m.res(VERSION_YDR_ENHANCED)).unwrap()[0].0, 0);
}

/// An RDR2 geometry of `count` vertices in streams: positions, then packed normals, then half uvs.
fn rdr2_geometry(m: &mut Mem, indices: &[u32]) -> u64 {
    let count = 2u32;
    let mut data = Vec::new();
    for x in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0] {
        data.extend(x.to_le_bytes());
    }
    data.extend(0xFFFF_FFFFu32.to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend([0x00, 0x3C, 0x00, 0x40, 0x00, 0x40, 0x00, 0x3C]);
    let decl = gen9_decl(m, &[(0, 0, 6), (4, 24, 24), (28, 32, 34)]);
    sga_geometry(m, Gen::Rdr2, (count, 20), &data, decl, indices, 2)
}

fn rdr2_part(m: &mut Mem, indices: &[u32]) -> u64 {
    let g = rdr2_geometry(m, indices);
    let md = model(m, &[g], None, 0x30, 0);
    let d = drawable_at(m, None, 0, &[md]);
    let group = group(m, &[]);
    m.ptr(d + 0x10, group);
    d
}

#[test]
fn rdr2_attributes_are_stored_as_separate_streams() {
    let mut m = Mem::new();
    let d = rdr2_part(&mut m, &[0, 1, 0]);
    let res = m.res(0);
    let d = read_drawable_gen(&res, d, Gen::Rdr2).unwrap();
    let mesh = &d.meshes[0];
    assert_eq!(mesh.positions, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
    assert_eq!(mesh.normals, [[1.0, 1.0, 1.0], [-1.0, -1.0, -1.0]], "normals are unsigned here");
    assert_eq!(mesh.uvs, [[1.0, 2.0], [2.0, 1.0]]);
    assert_eq!(mesh.indices, [0, 1, 0]);
}

#[test]
fn an_rdr2_stream_that_does_not_fit_the_buffer_is_left_out() {
    let mut m = Mem::new();
    let decl = gen9_decl(&mut m, &[(0, 0, 6), (28, 40, 34)]);
    let g = sga_geometry(&mut m, Gen::Rdr2, (2, 20), &[0u8; 40], decl, &[0, 1, 0], 2);
    let md = model(&mut m, &[g], None, 0x30, 0);
    let d = drawable_at(&mut m, None, 0, &[md]);
    let d = read_drawable_gen(&m.res(0), d, Gen::Rdr2).unwrap();
    assert_eq!(d.meshes[0].positions.len(), 2);
    assert!(d.meshes[0].uvs.is_empty(), "the uv stream lies past the end");
}

fn rdr2_skinned(table: Option<&[u16]>, model_tags: Option<&[u16]>) -> Mesh {
    let mut m = Mem::new();
    let mut data = Vec::new();
    for x in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0] {
        data.extend(x.to_le_bytes());
    }
    data.extend([255, 0, 0, 0, 255, 0, 0, 0]);
    data.extend([0, 1, 2, 3, 2, 2, 2, 2]);
    let decl = gen9_decl(&mut m, &[(0, 0, 6), (16, 24, 28), (20, 32, 30)]);
    let g = sga_geometry(&mut m, Gen::Rdr2, (2, 20), &data, decl, &[0, 1, 0], 2);
    if let Some(ids) = table {
        let at = m.bytes(&ids.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
        m.ptr(g + 0x18, at);
    }
    let md = model(&mut m, &[g], None, 0x30, 0);
    if let Some(tags) = model_tags {
        let at = m.bytes(&tags.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
        m.ptr(md + 0x28, at);
        m.u32(md + 0x30, 0x100 | tags.len() as u32);
    }
    let d = drawable_at(&mut m, None, 0, &[md]);
    let res = m.res(0);
    read_drawable_gen(&res, d, Gen::Rdr2).unwrap().meshes.remove(0)
}

#[test]
fn rdr2_blend_indices_go_through_the_geometry_table() {
    let mesh = rdr2_skinned(Some(&[10, 11, 12, 13]), None);
    assert_eq!(mesh.bone_indices, [[10, 11, 12, 13], [12, 12, 12, 12]]);
    assert_eq!(mesh.bone_weights[0], [1.0, 0.0, 0.0, 0.0]);
    assert!(mesh.bone_tags.is_empty());
}

#[test]
fn rdr2_blend_indices_without_a_table_stay() {
    let mesh = rdr2_skinned(None, None);
    assert_eq!(mesh.bone_indices, [[0, 1, 2, 3], [2, 2, 2, 2]]);
}

#[test]
fn an_rdr2_table_that_is_cut_off_leaves_the_indices() {
    let mut m = Mem::new();
    let g = rdr2_geometry(&mut m, &[0, 1, 0]);
    let mut mesh = Mesh { bone_indices: vec![[0, 1, 2, 3]], ..Default::default() };
    let at = SYS + m.0.len() as u64 - 4;
    m.ptr(g + 0x18, at);
    let res = m.res(0);
    remap_bones_rdr2(&res, g, &mut mesh).unwrap();
    assert_eq!(mesh.bone_indices, [[0, 1, 2, 3]]);
}

#[test]
fn a_skinned_rdr2_model_without_a_table_names_its_bones_by_tag() {
    let mesh = rdr2_skinned(None, Some(&[500, 501]));
    assert_eq!(mesh.bone_tags, [500, 501]);
    let mesh = rdr2_skinned(Some(&[1, 2, 3, 4]), Some(&[500, 501]));
    assert!(mesh.bone_tags.is_empty(), "a geometry with a table of its own uses it");
}

/// A fragment of RDR2: a body with one mesh, a skeleton, cloth and children (`wheel`: its group names the bone).
fn rdr2_fragment() -> Resource {
    let mut m = Mem::new();
    let body = rdr2_part(&mut m, &[0, 1, 0]);
    let shader = sga_shader(&mut m, 0x55, &[], 0, &[]);
    let g = group(&mut m, &[shader]);
    m.ptr(body + 0x10, g);
    let sk = skeleton(&mut m, true, &[("root", 100, -1, [0.0; 3]), ("wheel_lf", 200, 0, [1.0, 0.0, 0.0])]);
    m.ptr(body + 0x18, sk);
    m.ptr(SYS + 0x20, body);
    // Cloth: a list of 16-byte entries, each pointing at a cloth whose drawable is at 0x18.
    let cloth_drawable = rdr2_part(&mut m, &[0, 1, 0]);
    let cloth = m.alloc(0x20);
    m.ptr(cloth + 0x18, cloth_drawable);
    let list = m.alloc(32);
    m.ptr(list, cloth);
    m.ptr(list + 16, cloth);
    m.ptr(SYS + 0x48, list);
    m.u16(SYS + 0x50, 2);
    // Children: three drawables, three groups.
    let kids: Vec<u64> = (0..3).map(|_| rdr2_part(&mut m, &[0, 1, 0])).collect();
    let empty = m.alloc(0x80);
    let kids_list = m.alloc(8 * 5);
    for (i, k) in kids.iter().enumerate() {
        m.ptr(kids_list + i as u64 * 8, *k);
    }
    m.ptr(kids_list + 24, empty);
    m.ptr(kids_list + 32, kids[0]);
    let groups = m.alloc(24);
    for (i, (name, parent)) in [("wheel_lf", 0xFFu8), ("unnamed", 0), ("none", 0xFF)].iter().enumerate() {
        let gr = m.alloc(0x60);
        m.u8(gr + 6, *parent);
        m.put(gr + 0x4F, name.as_bytes());
        m.ptr(groups + i as u64 * 8, gr);
    }
    let group_of = m.bytes(&[0, 0, 1, 0, 2, 0, 0, 0, 0, 0]);
    let lod = m.alloc(0x150);
    m.ptr(lod + 0xC8, groups);
    m.ptr(lod + 0x110, group_of);
    m.ptr(lod + 0x118, kids_list);
    m.u8(lod + 0x140, 3);
    m.u8(lod + 0x146, 5);
    let physics = m.alloc(0x40);
    m.ptr(physics + 0x10, lod);
    m.ptr(SYS + 0x90, physics);
    m.res(0)
}

#[test]
fn an_rdr2_fragment_has_a_body_cloth_and_children() {
    let res = rdr2_fragment();
    let f = parse_fragment_rdr2(&res).unwrap();
    assert_eq!(f.drawable.meshes.len(), 1);
    assert_eq!(f.drawable.shaders.len(), 1);
    assert_eq!(f.drawable.skeleton.as_ref().unwrap().bones.len(), 2);
    assert_eq!(f.cloths.len(), 1, "a cloth listed twice is read once");
    assert_eq!(f.cloths[0].shaders[0].name_hash, 0x55, "a cloth without shaders uses the body's");
    assert_eq!(f.children.len(), 3);
    assert_eq!(f.children.iter().map(|c| c.bone_tag).collect::<Vec<_>>(), [200, 200, 100]);
    assert_eq!(f.children[0].drawable.shaders[0].name_hash, 0x55);
}

#[test]
fn an_rdr2_skeleton_can_be_read_without_the_meshes() {
    let res = rdr2_fragment();
    let s = fragment_skeleton_rdr2(&res).unwrap().unwrap();
    assert_eq!(s.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["root", "wheel_lf"]);
}

#[test]
fn an_rdr2_fragment_without_a_skeleton_has_none() {
    let mut res = rdr2_fragment();
    let body = res.u64(SYS + 0x20).unwrap();
    res.bytes_mut(body + 0x18, 8).unwrap().fill(0);
    assert!(fragment_skeleton_rdr2(&res).unwrap().is_none());
}

#[test]
fn an_rdr2_fragment_without_a_drawable_is_an_error() {
    let m = Mem::new();
    let res = m.res(0);
    assert_eq!(parse_fragment_rdr2(&res).unwrap_err().to_string(), "fragment without a drawable");
    assert!(fragment_skeleton_rdr2(&res).is_err());
}

#[test]
fn an_rdr2_body_is_found_among_the_roots_pointers() {
    let mut res = rdr2_fragment();
    let body = res.u64(SYS + 0x20).unwrap();
    res.bytes_mut(SYS + 0x20, 8).unwrap().fill(0);
    res.bytes_mut(SYS + 0x60, 8).unwrap().copy_from_slice(&body.to_le_bytes());
    assert_eq!(parse_fragment_rdr2(&res).unwrap().drawable.meshes.len(), 1);
}

#[test]
fn an_rdr2_fragment_that_is_all_cloth_keeps_its_bare_body() {
    let mut res = rdr2_fragment();
    let body = res.u64(SYS + 0x20).unwrap();
    res.bytes_mut(body + 0x50, 8).unwrap().fill(0);
    let f = parse_fragment_rdr2(&res).unwrap();
    assert!(f.drawable.meshes.is_empty());
    assert_eq!(f.drawable.skeleton.as_ref().unwrap().bones.len(), 2);
}

#[test]
fn an_rdr2_fragment_without_physics_has_no_children() {
    let mut res = rdr2_fragment();
    res.bytes_mut(SYS + 0x90, 8).unwrap().fill(0);
    assert!(parse_fragment_rdr2(&res).unwrap().children.is_empty());
}

#[test]
fn rdr2_drawable_dictionaries_and_drawables_are_read() {
    let mut m = Mem::new();
    let a = rdr2_part(&mut m, &[0, 1, 0]);
    let list = m.alloc(8);
    m.ptr(list, a);
    m.ptr(SYS + 0x30, list);
    m.u16(SYS + 0x38, 1);
    let res = m.res(0);
    let out = parse_ydd_rdr2(&res).unwrap();
    assert_eq!((out[0].0, out[0].1.meshes.len()), (0, 1));
    let mut m = Mem::new();
    let g = rdr2_geometry(&mut m, &[0, 1, 0]);
    let md = model(&mut m, &[g], None, 0x30, 0);
    drawable_at(&mut m, Some(SYS), 0, &[md]);
    assert_eq!(parse_ydr_rdr2(&m.res(0)).unwrap().meshes.len(), 1);
}
