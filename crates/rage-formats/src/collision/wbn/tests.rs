use super::*;

fn put(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A `.wbn` holding a geometry bound: a unit quad and a triangle (fourth index 0, off the plane).
fn wbn() -> Resource {
    let s = 0x5000_0000u32;
    let mut d = vec![0u8; 0x400];
    put(&mut d, 0x08, s + 0x40);
    let b = 0x40;
    d[b + 4] = kind::GEOMETRY;
    put(&mut d, b + 0x8C, s + 0x200);
    for k in 0..3 {
        put(&mut d, b + 0x90 + k * 4, 1.0f32.to_bits());
    }
    put(&mut d, b + 0xB0, s + 0x300);
    put(&mut d, b + 0xC8, 5);
    put(&mut d, b + 0xCC, 2);
    put(&mut d, b + 0xD0, s + 0x380);
    put(&mut d, b + 0xD8, 2);
    d[0x380] = 3;
    d[0x384] = 7;
    // vertices: (0,0,0) (1,0,0) (1,1,0) (0,1,0) (0,0,5)
    let verts: [[i16; 3]; 5] = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 5]];
    for (i, v) in verts.iter().enumerate() {
        for (k, c) in v.iter().enumerate() {
            d[0x300 + i * 6 + k * 2..0x300 + i * 6 + k * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    // quad 0-1-2-3, material 0, normal +z
    put(&mut d, 0x200 + 8, 1.0f32.to_bits());
    for (k, i) in [0u16, 1, 2, 3].iter().enumerate() {
        d[0x200 + 16 + k * 2..0x200 + 18 + k * 2].copy_from_slice(&i.to_le_bytes());
    }
    // triangle 1-2-4 (fourth index 0 is off its plane), material 1
    put(&mut d, 0x220, 1.0f32.to_bits());
    d[0x220 + 12] = 1;
    for (k, i) in [1u16, 2, 4, 0].iter().enumerate() {
        d[0x220 + 16 + k * 2..0x220 + 18 + k * 2].copy_from_slice(&i.to_le_bytes());
    }
    Resource { version: 32, system: d, graphics: Vec::new() }
}

#[test]
fn curved_bounds_become_cylinders_along_their_short_side() {
    let s = 0x5000_0000u32;
    let mut d = vec![0u8; 0x200];
    put(&mut d, 0x08, s + 0x40);
    let b = 0x40;
    d[b + 4] = kind::CURVED;
    for (o, v) in [(0x10, [0.1f32, 0.4, 0.4]), (0x20, [-0.1, -0.4, -0.4])] {
        for (k, c) in v.iter().enumerate() {
            put(&mut d, b + o + k * 4, c.to_bits());
        }
    }
    let (dr, stats) = parse_wbn(&Resource { version: 32, system: d, graphics: Vec::new() }).unwrap();
    assert_eq!(stats.primitives, 1);
    let (lo, hi) = (dr.bounds_min, dr.bounds_max);
    assert!((hi[0] - 0.1).abs() < 1e-4 && (lo[0] + 0.1).abs() < 1e-4, "{lo:?} {hi:?}");
    assert!((hi[1] - 0.4).abs() < 0.02 && (hi[2] - 0.4).abs() < 0.02, "{hi:?}");
}

#[test]
fn reads_quads_and_triangles() {
    let (d, stats) = parse_wbn(&wbn()).unwrap();
    assert_eq!(stats.triangles, 3);
    assert_eq!(stats.materials, vec![3, 7]);
    let hashes: Vec<u32> = d.shaders.iter().map(|s| s.name_hash).collect();
    assert_eq!(hashes, vec![MATERIAL_BASE + 3, MATERIAL_BASE + 7]);
}

const S: u32 = 0x5000_0000;

fn put_f32(d: &mut [u8], at: usize, v: f32) {
    put(d, at, v.to_bits());
}

fn put_vec3(d: &mut [u8], at: usize, v: [f32; 3]) {
    for (k, x) in v.iter().enumerate() {
        put_f32(d, at + k * 4, *x);
    }
}

fn resource(d: Vec<u8>) -> Resource {
    Resource { version: 32, system: d, graphics: Vec::new() }
}

/// A `.wbn` whose bound at 0x40 is set up by `f`.
fn file_with(f: impl FnOnce(&mut Vec<u8>, usize)) -> Resource {
    let mut d = vec![0u8; 0x800];
    put(&mut d, 0x08, S + 0x40);
    f(&mut d, 0x40);
    resource(d)
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn assert_extent(d: &Drawable, lo: [f32; 3], hi: [f32; 3]) {
    assert!(near(d.bounds_min, lo) && near(d.bounds_max, hi), "{:?} {:?}", d.bounds_min, d.bounds_max);
}

/// A polyhedron of `kind` at `b` with the vertices and 32-byte polygons given.
fn polyhedron(d: &mut [u8], b: usize, bound_kind: u8, verts: &[[i16; 3]], polys: &[[u8; 32]], types: &[u8]) {
    d[b + 4] = bound_kind;
    put(d, b + 0x8C, S + 0x200);
    put_vec3(d, b + 0x90, [1.0; 3]);
    put(d, b + 0xB0, S + 0x300);
    put(d, b + 0xC8, verts.len() as u32);
    put(d, b + 0xCC, polys.len() as u32);
    put(d, b + 0xD0, S + 0x380);
    put(d, b + 0xD8, types.len() as u32);
    for (i, t) in types.iter().enumerate() {
        d[0x380 + i * 4] = *t;
    }
    for (i, v) in verts.iter().enumerate() {
        for (k, c) in v.iter().enumerate() {
            d[0x300 + i * 6 + k * 2..0x300 + i * 6 + k * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    for (i, p) in polys.iter().enumerate() {
        d[0x200 + i * 32..0x200 + i * 32 + 32].copy_from_slice(p);
    }
}

fn polygon(material: u8, idx: [u16; 4]) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[8..12].copy_from_slice(&1.0f32.to_le_bytes());
    p[12] = material;
    for (k, i) in idx.iter().enumerate() {
        p[16 + k * 2..18 + k * 2].copy_from_slice(&i.to_le_bytes());
    }
    p
}

#[test]
fn a_sphere_is_drawn_around_its_centre() {
    let res = file_with(|d, b| {
        d[b + 4] = kind::SPHERE;
        put_f32(d, b + 0x08, 2.0);
        put_vec3(d, b + 0x30, [1.0, 2.0, 3.0]);
    });
    let (d, stats) = parse_wbn(&res).unwrap();
    assert_eq!((stats.bounds, stats.primitives, stats.triangles), (1, 1, 0));
    assert_eq!(stats.materials, [0]);
    assert_eq!(d.shaders[0].name_hash, MATERIAL_BASE);
    assert_extent(&d, [-1.0, 0.0, 1.0], [3.0, 4.0, 5.0]);
}

#[test]
fn a_capsule_runs_along_y() {
    let res = file_with(|d, b| {
        d[b + 4] = kind::CAPSULE;
        put_f32(d, b + 0x80, 1.0);
        put_f32(d, b + 0x90, 4.0);
        put_vec3(d, b + 0x30, [0.0, 10.0, 0.0]);
    });
    let (d, stats) = parse_wbn(&res).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_extent(&d, [-1.0, 7.0, -1.0], [1.0, 13.0, 1.0]);
}

#[test]
fn a_curved_bound_takes_its_material_from_the_list() {
    let res = file_with(|d, b| {
        d[b + 4] = kind::CURVED;
        put_vec3(d, b + 0x10, [0.1, 0.4, 0.4]);
        put_vec3(d, b + 0x20, [-0.1, -0.4, -0.4]);
        put(d, b + 0xD0, S + 0x380);
        d[0x380] = 6;
    });
    let (_, stats) = parse_wbn(&res).unwrap();
    assert_eq!(stats.materials, [6]);
}

#[test]
fn a_box_has_no_material_list() {
    let verts = [[0, 0, 0], [4, 0, 0], [4, 4, 0], [0, 4, 0]];
    let res = file_with(|d, b| {
        polyhedron(d, b, kind::BOX, &verts, &[polygon(1, [0, 1, 2, 3])], &[8, 9]);
    });
    let (_, stats) = parse_wbn(&res).unwrap();
    assert_eq!((stats.triangles, stats.vertices), (2, 4));
    assert_eq!(stats.materials, [0]);
}

#[test]
fn polygons_get_the_material_of_their_index() {
    let verts = [[0, 0, 0], [4, 0, 0], [4, 4, 0], [0, 4, 0]];
    let polys = [polygon(1, [0, 1, 2, 3]), polygon(9, [0, 1, 2, 3])];
    let res = file_with(|d, b| polyhedron(d, b, kind::BVH, &verts, &polys, &[8, 9]));
    let (d, stats) = parse_wbn(&res).unwrap();
    assert_eq!(stats.materials, [8, 9], "an index beyond the list falls back to the first type");
    assert_eq!(d.shaders.iter().map(|s| s.name_hash).collect::<Vec<_>>(), [MATERIAL_BASE + 9, MATERIAL_BASE + 8]);
    assert_eq!(stats.triangles, 4);
}

#[test]
fn a_polygon_naming_a_missing_vertex_is_left_out() {
    let verts = [[0, 0, 0], [4, 0, 0], [4, 4, 0]];
    let polys = [polygon(0, [0, 1, 2, 3]), polygon(0, [0, 1, 2, 2])];
    let res = file_with(|d, b| polyhedron(d, b, kind::GEOMETRY, &verts, &polys, &[1]));
    let (_, stats) = parse_wbn(&res).unwrap();
    assert_eq!(stats.triangles, 2, "only the second polygon counts, as two triangles");
}

#[test]
fn an_empty_material_count_still_has_one_type() {
    let verts = [[0, 0, 0], [4, 0, 0], [4, 4, 0], [0, 4, 0]];
    let res = file_with(|d, b| {
        polyhedron(d, b, kind::GEOMETRY, &verts, &[polygon(0, [0, 1, 2, 3])], &[5]);
        put(d, b + 0xD8, 0);
    });
    let (_, stats) = parse_wbn(&res).unwrap();
    assert_eq!(stats.materials, [5]);
    let res = file_with(|d, b| {
        polyhedron(d, b, kind::GEOMETRY, &verts, &[polygon(0, [0, 1, 2, 3])], &[5]);
        put(d, b + 0xD0, 0);
    });
    assert_eq!(parse_wbn(&res).unwrap().1.materials, [0], "no list: type 0");
}

#[test]
fn vertices_use_quantum_and_offset() {
    let verts = [[0, 0, 0], [10, 0, 0], [0, 10, 0]];
    let res = file_with(|d, b| {
        polyhedron(d, b, kind::GEOMETRY, &verts, &[polygon(0, [0, 1, 2, 1])], &[1]);
        put_vec3(d, b + 0x90, [0.5, 0.25, 1.0]);
        put_vec3(d, b + 0xA0, [100.0, 200.0, 300.0]);
    });
    let (d, _) = parse_wbn(&res).unwrap();
    assert_eq!(d.meshes[0].positions[..3], [[100.0, 200.0, 300.0], [105.0, 200.0, 300.0], [100.0, 202.5, 300.0]]);
}

#[test]
fn an_implausible_geometry_is_refused() {
    let res = file_with(|d, b| {
        polyhedron(d, b, kind::GEOMETRY, &[], &[], &[1]);
        put(d, b + 0xC8, (1 << 20) + 1);
    });
    assert_eq!(parse_wbn(&res).err().unwrap().to_string(), "implausible geometry: 1048577 vertices, 0 polygons");
    let res = file_with(|d, b| {
        polyhedron(d, b, kind::GEOMETRY, &[], &[], &[1]);
        put(d, b + 0xCC, (1 << 21) + 1);
    });
    assert_eq!(parse_wbn(&res).err().unwrap().to_string(), "implausible geometry: 0 vertices, 2097153 polygons");
}

#[test]
fn a_geometry_without_polygons_is_empty() {
    let res = file_with(|d, b| polyhedron(d, b, kind::GEOMETRY, &[], &[], &[1]));
    let (d, stats) = parse_wbn(&res).unwrap();
    assert_eq!((stats.bounds, stats.triangles, stats.vertices), (1, 0, 0));
    assert!(d.meshes.is_empty());
}

#[test]
fn an_unknown_bound_type_is_an_error() {
    let res = file_with(|d, b| d[b + 4] = 7);
    assert_eq!(parse_wbn(&res).err().unwrap().to_string(), "unknown bound type 7");
}

#[test]
fn a_file_without_a_bound_is_an_error() {
    let res = resource(vec![0u8; 0x40]);
    assert_eq!(parse_wbn(&res).err().unwrap().to_string(), "collision file without a bound");
}

#[test]
fn a_truncated_bound_is_an_error() {
    let mut res = file_with(|d, b| {
        d[b + 4] = kind::SPHERE;
        put_f32(d, b + 0x08, 1.0);
    });
    res.system.truncate(0x40 + 0x34);
    assert!(matches!(parse_wbn(&res), Err(ResourceError::OutOfBounds { .. })));
    res.system.truncate(0x42);
    assert!(matches!(parse_wbn(&res), Err(ResourceError::OutOfBounds { .. })));
}

fn matrix(d: &mut [u8], at: usize, translation: [f32; 3]) {
    for k in 0..3 {
        put_f32(d, at + k * 16 + k * 4, 1.0);
    }
    put_f32(d, at + 12, 77.0);
    put_vec3(d, at + 48, translation);
}

fn sphere(d: &mut [u8], at: usize, centre: [f32; 3], radius: f32) {
    d[at + 4] = kind::SPHERE;
    put_f32(d, at + 0x08, radius);
    put_vec3(d, at + 0x30, centre);
}

#[test]
fn composite_children_are_moved_by_their_matrices() {
    let res = file_with(|d, b| {
        d[b + 4] = kind::COMPOSITE;
        put(d, b + 0x80, S + 0x100);
        put(d, b + 0x84, S + 0x400);
        d[b + 0x90] = 3;
        put(d, 0x100, S + 0x180);
        put(d, 0x104, S + 0x1C0);
        sphere(d, 0x180, [0.0; 3], 1.0);
        sphere(d, 0x1C0, [0.0; 3], 1.0);
        matrix(d, 0x400, [10.0, 0.0, 0.0]);
        matrix(d, 0x400 + 64, [0.0, 0.0, 30.0]);
    });
    let (d, stats) = parse_wbn(&res).unwrap();
    assert_eq!((stats.bounds, stats.primitives), (3, 2));
    assert_extent(&d, [-1.0, -1.0, -1.0], [11.0, 1.0, 31.0]);
}

#[test]
fn composite_children_without_matrices_stay_in_place() {
    let res = file_with(|d, b| {
        d[b + 4] = kind::COMPOSITE;
        put(d, b + 0x80, S + 0x100);
        d[b + 0x90] = 1;
        put(d, 0x100, S + 0x180);
        sphere(d, 0x180, [5.0, 0.0, 0.0], 1.0);
    });
    let (d, _) = parse_wbn(&res).unwrap();
    assert_extent(&d, [4.0, -1.0, -1.0], [6.0, 1.0, 1.0]);
}

#[test]
fn a_composite_containing_itself_is_too_deep() {
    let res = file_with(|d, b| {
        d[b + 4] = kind::COMPOSITE;
        put(d, b + 0x80, S + 0x100);
        d[b + 0x90] = 1;
        put(d, 0x100, S + 0x40);
    });
    assert_eq!(parse_wbn(&res).err().unwrap().to_string(), "bounds nested too deeply");
}

fn dictionary(hashes: bool, count: u16, children: &[u32]) -> Resource {
    let mut d = vec![0u8; 0x800];
    if hashes {
        put(&mut d, 0x10, S + 0x100);
    }
    put(&mut d, 0x18, S + 0x140);
    d[0x1C..0x1E].copy_from_slice(&count.to_le_bytes());
    for (i, c) in children.iter().enumerate() {
        put(&mut d, 0x100 + i * 4, 0xA000 + i as u32);
        put(&mut d, 0x140 + i * 4, if *c == 0 { 0 } else { S + c });
    }
    sphere(&mut d, 0x200, [0.0; 3], 1.0);
    sphere(&mut d, 0x300, [0.0; 3], 2.0);
    resource(d)
}

#[test]
fn a_dictionary_pairs_hashes_with_bounds() {
    let out = parse_wbd(&dictionary(true, 3, &[0x200, 0, 0x300])).unwrap();
    assert_eq!(out.len(), 2, "empty entries are skipped");
    assert_eq!((out[0].0, out[1].0), (0xA000, 0xA002));
    assert_extent(&out[0].1, [-1.0; 3], [1.0; 3]);
    assert_extent(&out[1].1, [-2.0; 3], [2.0; 3]);
    assert_eq!(out[1].2.bounds, 1);
}

#[test]
fn a_dictionary_without_hashes_gives_zero() {
    let out = parse_wbd(&dictionary(false, 1, &[0x200])).unwrap();
    assert_eq!(out[0].0, 0);
}

#[test]
fn an_empty_dictionary_has_no_bounds() {
    assert!(parse_wbd(&dictionary(true, 0, &[])).unwrap().is_empty());
}

#[test]
fn an_implausible_dictionary_is_refused() {
    let message = parse_wbd(&dictionary(true, (1 << 14) + 1, &[])).err().unwrap().to_string();
    assert_eq!(message, "implausible dictionary of 16385 bounds");
}

#[test]
fn a_truncated_dictionary_is_an_error() {
    let mut res = dictionary(true, 2, &[0x200, 0x300]);
    res.system.truncate(0x1E);
    assert!(matches!(parse_wbd(&res), Err(ResourceError::OutOfBounds { .. })));
}
