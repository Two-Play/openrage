use super::*;

const SYS: u64 = 0x5000_0000;
const LIST: usize = 0x600;
const MATRICES: usize = 0x700;

fn put(d: &mut [u8], at: usize, bytes: &[u8]) {
    d[at..at + bytes.len()].copy_from_slice(bytes);
}

fn put_f32(d: &mut [u8], at: usize, v: f32) {
    put(d, at, &v.to_le_bytes());
}

fn put_vec3(d: &mut [u8], at: usize, v: [f32; 3]) {
    for (k, x) in v.iter().enumerate() {
        put_f32(d, at + k * 4, *x);
    }
}

fn put_ptr(d: &mut [u8], at: usize, to: usize) {
    put(d, at, &(SYS + to as u64).to_le_bytes());
}

fn resource(d: Vec<u8>) -> Resource {
    Resource { version: 159, system: d, graphics: Vec::new() }
}

fn memory() -> Vec<u8> {
    vec![0u8; 0x1000]
}

/// A sphere bound at `at`.
fn sphere(d: &mut [u8], at: usize, centre: [f32; 3], radius: f32, material: u8) {
    d[at + 0x10] = kind::SPHERE;
    d[at + 0x4C] = material;
    put_f32(d, at + 0x14, radius);
    put_vec3(d, at + 0x50, centre);
}

fn cuboid(d: &mut [u8], at: usize, lo: [f32; 3], hi: [f32; 3]) {
    d[at + 0x10] = kind::BOX;
    put_vec3(d, at + 0x30, lo);
    put_vec3(d, at + 0x20, hi);
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn assert_extent(d: &Drawable, lo: [f32; 3], hi: [f32; 3]) {
    assert!(near(d.bounds_min, lo) && near(d.bounds_max, hi), "{:?} {:?}", d.bounds_min, d.bounds_max);
}

/// A geometry bound (`bound_kind`) at 0 with `verts`, 16-byte `polys`, the material
/// types and the material index of each polygon; quantum 1, centre 0.
fn geometry(bound_kind: u8, verts: &[[i16; 3]], polys: &[[u8; 16]], types: &[u8], indices: &[u8]) -> Resource {
    let mut d = memory();
    d[0x10] = bound_kind;
    put_ptr(&mut d, 0x88, 0x200);
    put_vec3(&mut d, 0x90, [1.0; 3]);
    put_ptr(&mut d, 0xB0, 0x400);
    put(&mut d, 0xD0, &(verts.len() as u32).to_le_bytes());
    put(&mut d, 0xD4, &(polys.len() as u32).to_le_bytes());
    put_ptr(&mut d, 0xF0, 0x500);
    put_ptr(&mut d, 0x118, 0x520);
    d[0x120] = types.len() as u8;
    for (i, v) in verts.iter().enumerate() {
        for (k, c) in v.iter().enumerate() {
            put(&mut d, 0x400 + i * 6 + k * 2, &c.to_le_bytes());
        }
    }
    for (i, p) in polys.iter().enumerate() {
        put(&mut d, 0x200 + i * 16, p);
    }
    for (i, t) in types.iter().enumerate() {
        d[0x500 + i * 8] = *t;
    }
    put(&mut d, 0x520, indices);
    resource(d)
}

fn triangle_poly(a: u16, b: u16, c: u16) -> [u8; 16] {
    let mut p = [0u8; 16];
    for (k, v) in [a, b, c].iter().enumerate() {
        put(&mut p, 4 + k * 2, &v.to_le_bytes());
    }
    p
}

fn round_poly(poly_kind: u8, a: u16, b: u16, radius: f32) -> [u8; 16] {
    let mut p = [0u8; 16];
    p[0] = poly_kind;
    put(&mut p, 2, &a.to_le_bytes());
    put_f32(&mut p, 4, radius);
    put(&mut p, 8, &b.to_le_bytes());
    p
}

#[test]
fn reads_material_names() {
    let dat = "12.00\n# comment\nDEFAULT\t\t\tDefault\t\tVOID\tNONE\tNONE\tNONE\t0.5\t0.1\t1.0\t1.0\n\
               CONCRETE\t\tConcrete\tCONCRETE\tNONE\tNONE\tNONE\t0.7\t0.1\t2.0\t1.0\n";
    assert_eq!(material_names(dat), vec!["DEFAULT", "CONCRETE"]);
}

#[test]
fn material_names_skip_short_lines_comments_and_the_header() {
    let dat = "HEADER A B C D E F G H I J K L\n\
               # COMMENT LINE WITH MANY WORDS HERE A B C D E F\n\
               SHORT A B\n\
               NINE_FIELDS A B C D E F G H\n\
               SPACED   Name  A  B  C  D  E  F  G  H\n";
    assert_eq!(material_names(dat), vec!["SPACED"]);
    assert!(material_names("").is_empty());
}

#[test]
fn a_sphere_bound_becomes_a_sphere() {
    let mut d = memory();
    sphere(&mut d, 0, [1.0, 2.0, 3.0], 2.0, 6);
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!((stats.bounds, stats.primitives, stats.triangles), (1, 1, 0));
    assert_eq!(stats.materials, [6]);
    assert_extent(&dr, [-1.0, 0.0, 1.0], [3.0, 4.0, 5.0]);
    assert_eq!(dr.name, "collision");
    assert_eq!(dr.shaders[0].name_hash, MATERIAL_BASE + 6);
    assert_eq!(dr.meshes[0].shader, 0);
}

#[test]
fn a_box_bound_spans_its_corners() {
    let mut d = memory();
    cuboid(&mut d, 0, [-1.0, -2.0, -3.0], [4.0, 5.0, 6.0]);
    d[0x4C] = 2;
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!((stats.bounds, stats.primitives), (1, 1));
    assert_eq!(dr.meshes[0].indices.len(), 36);
    assert_eq!(stats.materials, [2]);
    assert_extent(&dr, [-1.0, -2.0, -3.0], [4.0, 5.0, 6.0]);
}

#[test]
fn a_capsule_bound_reaches_its_radius_beyond_the_straight_part() {
    let mut d = memory();
    d[0x10] = kind::CAPSULE;
    put_f32(&mut d, 0x14, 3.0);
    put_f32(&mut d, 0x2C, 1.0);
    put_vec3(&mut d, 0x40, [10.0, 0.0, 0.0]);
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_extent(&dr, [9.0, -3.0, -1.0], [11.0, 3.0, 1.0]);
}

#[test]
fn a_cylinder_bound_stands_along_y() {
    let mut d = memory();
    d[0x10] = kind::CYLINDER;
    put_vec3(&mut d, 0x30, [-1.0, -3.0, -1.0]);
    put_vec3(&mut d, 0x20, [1.0, 3.0, 1.0]);
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_extent(&dr, [-1.0, -3.0, -1.0], [1.0, 3.0, 1.0]);
}

#[test]
fn a_disc_bound_lies_along_x() {
    let mut d = memory();
    d[0x10] = kind::DISC;
    put_f32(&mut d, 0x14, 2.0);
    put_f32(&mut d, 0x2C, 0.5);
    put_vec3(&mut d, 0x30, [-0.5, -2.0, -2.0]);
    put_vec3(&mut d, 0x20, [0.5, 2.0, 2.0]);
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_extent(&dr, [-0.5, -2.0, -2.0], [0.5, 2.0, 2.0]);
}

#[test]
fn a_bound_of_an_unknown_kind_adds_nothing() {
    let mut d = memory();
    d[0x10] = 2;
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!((stats.bounds, stats.primitives, stats.triangles), (1, 0, 0));
    assert!(stats.materials.is_empty() && dr.meshes.is_empty() && dr.shaders.is_empty());
    assert_eq!((dr.bounds_min, dr.bounds_max), ([0.0; 3], [0.0; 3]));
}

#[test]
fn triangles_are_grouped_by_material_type() {
    let verts = [[0, 0, 0], [4, 0, 0], [0, 4, 0], [0, 0, 4]];
    let polys = [triangle_poly(0, 1, 2), triangle_poly(0x8000, 2, 3), triangle_poly(1, 3, 2)];
    let res = geometry(kind::GEOMETRY, &verts, &polys, &[7, 9], &[0, 1, 0]);
    let (d, stats) = parse(&res).unwrap();
    assert_eq!((stats.bounds, stats.triangles, stats.vertices, stats.primitives), (1, 3, 4, 0));
    assert_eq!(stats.materials, [7, 9]);
    assert_eq!(d.shaders.iter().map(|s| s.name_hash).collect::<Vec<_>>(), [MATERIAL_BASE + 7, MATERIAL_BASE + 9]);
    assert_eq!(d.meshes[0].positions.len(), 6);
    assert_eq!(
        d.meshes[1].positions,
        [[0.0; 3], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
        "the flag bit of an index is ignored"
    );
    assert_eq!(d.meshes[0].normals[0], [0.0, 0.0, 1.0]);
    assert_eq!(d.meshes[1].shader, 1);
    assert_extent(&d, [0.0; 3], [4.0, 4.0, 4.0]);
}

#[test]
fn a_bvh_bound_reads_like_a_geometry_bound() {
    let verts = [[0, 0, 0], [4, 0, 0], [0, 4, 0]];
    let res = geometry(kind::BVH, &verts, &[triangle_poly(0, 1, 2)], &[3], &[0]);
    let (_, stats) = parse(&res).unwrap();
    assert_eq!((stats.triangles, stats.materials), (1, vec![3]));
}

#[test]
fn a_triangle_naming_a_missing_vertex_is_left_out() {
    let verts = [[0, 0, 0], [4, 0, 0], [0, 4, 0]];
    let res = geometry(kind::GEOMETRY, &verts, &[triangle_poly(0, 1, 3), triangle_poly(0, 1, 2)], &[1], &[0, 0]);
    let (d, stats) = parse(&res).unwrap();
    assert_eq!(stats.triangles, 1);
    assert_eq!(d.meshes[0].indices.len(), 3);
}

#[test]
fn polygons_without_a_material_list_use_type_zero() {
    let verts = [[0, 0, 0], [4, 0, 0], [0, 4, 0]];
    let mut res = geometry(kind::GEOMETRY, &verts, &[triangle_poly(0, 1, 2)], &[5], &[3]);
    let (_, stats) = parse(&res).unwrap();
    assert_eq!(stats.materials, [0], "an index outside the list gives type 0");
    res.system[0xF0..0xF8].fill(0);
    res.system[0x120] = 0;
    assert_eq!(parse(&res).unwrap().1.materials, [0]);
    res.system[0x118..0x120].fill(0);
    assert_eq!(parse(&res).unwrap().1.triangles, 1);
}

#[test]
fn sphere_polygons_are_spheres() {
    let res = geometry(kind::GEOMETRY, &[[1, 2, 3]], &[round_poly(1, 0, 0, 2.0)], &[4], &[0]);
    let (d, stats) = parse(&res).unwrap();
    assert_eq!((stats.primitives, stats.triangles), (1, 0));
    assert_extent(&d, [-1.0, 0.0, 1.0], [3.0, 4.0, 5.0]);
}

#[test]
fn capsule_and_cylinder_polygons_run_between_two_vertices() {
    let verts = [[0, 0, 0], [0, 0, 6]];
    let res = geometry(kind::GEOMETRY, &verts, &[round_poly(2, 0, 1, 1.0)], &[4], &[0]);
    let (d, stats) = parse(&res).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_extent(&d, [-1.0, -1.0, -1.0], [1.0, 1.0, 7.0]);
    let res = geometry(kind::GEOMETRY, &verts, &[round_poly(4, 0, 1, 1.0)], &[4], &[0]);
    let (d, stats) = parse(&res).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_extent(&d, [-1.0, -1.0, 0.0], [1.0, 1.0, 6.0]);
}

#[test]
fn a_capsule_polygon_naming_a_missing_vertex_is_left_out() {
    let res = geometry(kind::GEOMETRY, &[[0, 0, 0]], &[round_poly(2, 0, 1, 1.0)], &[4], &[0]);
    let (d, stats) = parse(&res).unwrap();
    assert_eq!(stats.primitives, 0);
    assert!(d.meshes.is_empty());
}

#[test]
fn box_polygons_are_boxes_through_four_alternating_corners() {
    let verts = [[1, 1, 1], [1, -1, -1], [-1, 1, -1], [-1, -1, 1]];
    let mut p = [0u8; 16];
    p[0] = 3;
    for k in 0..4u16 {
        put(&mut p, 4 + k as usize * 2, &k.to_le_bytes());
    }
    let (d, stats) = parse(&geometry(kind::GEOMETRY, &verts, &[p], &[4], &[0])).unwrap();
    assert_eq!(stats.primitives, 1);
    assert_eq!(d.meshes[0].indices.len(), 36);
    assert_extent(&d, [-1.0; 3], [1.0; 3]);
}

#[test]
fn polygon_types_beyond_the_known_are_ignored() {
    let verts = [[0, 0, 0], [4, 0, 0], [0, 4, 0]];
    let mut p = triangle_poly(0, 1, 2);
    p[0] = 5;
    let (d, stats) = parse(&geometry(kind::GEOMETRY, &verts, &[p], &[4], &[0])).unwrap();
    assert_eq!((stats.triangles, stats.primitives, stats.vertices), (0, 0, 3));
    assert!(d.meshes.is_empty());
}

#[test]
fn vertices_use_quantum_and_centre() {
    let mut res = geometry(kind::GEOMETRY, &[[0, 0, 0], [10, 0, 0], [0, 10, 0]], &[triangle_poly(0, 1, 2)], &[1], &[0]);
    put_vec3(&mut res.system, 0x90, [0.5, 0.25, 1.0]);
    put_vec3(&mut res.system, 0xA0, [100.0, 200.0, 300.0]);
    let (d, _) = parse(&res).unwrap();
    assert_eq!(d.meshes[0].positions, [[100.0, 200.0, 300.0], [105.0, 200.0, 300.0], [100.0, 202.5, 300.0]]);
}

#[test]
fn an_implausible_vertex_count_is_refused() {
    let mut res = geometry(kind::GEOMETRY, &[], &[], &[1], &[]);
    put(&mut res.system, 0xD0, &((1u32 << 20) + 1).to_le_bytes());
    let message = parse(&res).err().unwrap().to_string();
    assert_eq!(message, "implausible geometry: 1048577 vertices, 0 polygons");
    put(&mut res.system, 0xD0, &0u32.to_le_bytes());
    put(&mut res.system, 0xD4, &((1u32 << 21) + 1).to_le_bytes());
    let message = parse(&res).err().unwrap().to_string();
    assert_eq!(message, "implausible geometry: 0 vertices, 2097153 polygons");
}

#[test]
fn a_geometry_without_vertices_or_polygons_is_empty() {
    let (d, stats) = parse(&geometry(kind::GEOMETRY, &[], &[], &[1], &[])).unwrap();
    assert_eq!((stats.bounds, stats.vertices, stats.triangles), (1, 0, 0));
    assert!(d.meshes.is_empty());
}

#[test]
fn pointers_out_of_the_resource_are_errors() {
    let verts = [[0, 0, 0], [4, 0, 0], [0, 4, 0]];
    let mut res = geometry(kind::GEOMETRY, &verts, &[triangle_poly(0, 1, 2)], &[1], &[0]);
    put(&mut res.system, 0x88, &(SYS + 0xFFFF_0000).to_le_bytes());
    assert!(matches!(parse(&res), Err(ResourceError::OutOfBounds { .. })));
    let mut res = geometry(kind::GEOMETRY, &verts, &[triangle_poly(0, 1, 2)], &[1], &[0]);
    put(&mut res.system, 0xB0, &0x1234u64.to_le_bytes());
    assert!(matches!(parse(&res), Err(ResourceError::BadPointer(0x1234))));
}

#[test]
fn a_truncated_header_is_an_error() {
    let mut d = memory();
    sphere(&mut d, 0, [0.0; 3], 1.0, 0);
    d.truncate(0x54);
    assert!(matches!(parse(&resource(d)), Err(ResourceError::OutOfBounds { .. })));
    assert!(matches!(parse(&resource(Vec::new())), Err(ResourceError::OutOfBounds { .. })));
}

fn matrix(d: &mut [u8], i: usize, translation: [f32; 3]) {
    let at = MATRICES + i * 64;
    for k in 0..3 {
        put_f32(d, at + k * 16 + k * 4, 1.0);
    }
    for (k, w) in [3, 7, 11].iter().enumerate() {
        put_f32(d, at + w * 4, 99.0 + k as f32);
    }
    put_vec3(d, at + 48, translation);
    put_f32(d, at + 60, 5.0);
}

fn composite(d: &mut [u8], at: usize, children: &[usize], matrices: Option<usize>) {
    d[at + 0x10] = kind::COMPOSITE;
    put_ptr(d, at + 0x70, LIST);
    if let Some(m) = matrices {
        put_ptr(d, at + 0x78, m);
    }
    put(d, at + 0xA0, &(children.len() as u16).to_le_bytes());
    for (i, c) in children.iter().enumerate() {
        if *c != 0 {
            put_ptr(d, LIST + i * 8, *c);
        }
    }
}

#[test]
fn composite_children_are_moved_by_their_transforms() {
    let mut d = memory();
    composite(&mut d, 0, &[0x100, 0, 0x180], Some(MATRICES));
    sphere(&mut d, 0x100, [0.0; 3], 1.0, 1);
    cuboid(&mut d, 0x180, [-1.0; 3], [1.0; 3]);
    matrix(&mut d, 0, [10.0, 0.0, 0.0]);
    matrix(&mut d, 2, [0.0, 0.0, 20.0]);
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!((stats.bounds, stats.primitives), (3, 2));
    assert_extent(&dr, [-1.0, -1.0, -1.0], [11.0, 1.0, 21.0]);
}

#[test]
fn composite_children_without_transforms_stay_in_place() {
    let mut d = memory();
    composite(&mut d, 0, &[0x100], None);
    sphere(&mut d, 0x100, [5.0, 0.0, 0.0], 1.0, 1);
    let (dr, _) = parse(&resource(d)).unwrap();
    assert_extent(&dr, [4.0, -1.0, -1.0], [6.0, 1.0, 1.0]);
}

#[test]
fn nested_composites_add_their_transforms() {
    let mut d = memory();
    composite(&mut d, 0, &[0x100], Some(MATRICES));
    matrix(&mut d, 0, [1.0, 0.0, 0.0]);
    d[0x100 + 0x10] = kind::COMPOSITE;
    put_ptr(&mut d, 0x100 + 0x70, 0x650);
    put_ptr(&mut d, 0x100 + 0x78, 0x800);
    put(&mut d, 0x100 + 0xA0, &1u16.to_le_bytes());
    put_ptr(&mut d, 0x650, 0x180);
    let inner = MATRICES + 0x100;
    for k in 0..3 {
        put_f32(&mut d, inner + k * 16 + k * 4, 1.0);
    }
    put_vec3(&mut d, inner + 48, [0.0, 2.0, 0.0]);
    sphere(&mut d, 0x180, [0.0; 3], 1.0, 1);
    let (dr, stats) = parse(&resource(d)).unwrap();
    assert_eq!(stats.bounds, 3);
    assert_extent(&dr, [0.0, 1.0, -1.0], [2.0, 3.0, 1.0]);
}

#[test]
fn a_composite_containing_itself_is_too_deep() {
    let mut d = memory();
    composite(&mut d, 0, &[0], None);
    put(&mut d, LIST, &SYS.to_le_bytes());
    let message = parse(&resource(d)).err().unwrap().to_string();
    assert_eq!(message, "bounds nested too deeply");
}

#[test]
fn a_composite_with_a_child_out_of_the_resource_is_an_error() {
    let mut d = memory();
    composite(&mut d, 0, &[0x100], Some(MATRICES));
    put(&mut d, LIST, &(SYS + 0x10_0000).to_le_bytes());
    assert!(matches!(parse(&resource(d)), Err(ResourceError::OutOfBounds { .. })));
}

#[test]
fn bounds_are_read_where_a_model_embeds_them() {
    let mut d = memory();
    sphere(&mut d, 0x300, [0.0, 0.0, 9.0], 1.0, 3);
    let res = resource(d);
    let (dr, stats) = parse_at(&res, SYS + 0x300).unwrap();
    assert_eq!(stats.materials, [3]);
    assert_extent(&dr, [-1.0, -1.0, 8.0], [1.0, 1.0, 10.0]);
    assert_eq!(parse(&res).unwrap().1.bounds, 1);
    assert!(parse_at(&res, 0x10).is_err());
}

#[test]
fn finished_collision_has_one_shader_per_material() {
    let mut b = Builder::default();
    b.triangle(5, &math::IDENTITY, [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    b.triangle(2, &math::IDENTITY, [[0.0; 3], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]]);
    b.primitives = 4;
    let (d, stats) = finish(b, Stats::default());
    assert_eq!(stats.materials, [2, 5], "sorted");
    assert_eq!(stats.primitives, 4);
    assert_eq!(d.shaders[0].name_hash, MATERIAL_BASE + 5, "in the order of the meshes");
    assert_eq!(d.shaders[1].name_hash, MATERIAL_BASE + 2);
    assert_extent(&d, [0.0; 3], [2.0, 3.0, 0.0]);
}
