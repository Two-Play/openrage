use super::*;

/// A composite with one geometry: a triangle and a capsule.
fn collision() -> Vec<u8> {
    let mut d = vec![0u8; 0x400];
    let put = |d: &mut Vec<u8>, at: usize, bytes: &[u8]| d[at..at + bytes.len()].copy_from_slice(bytes);
    let pointer = |to: u64| (SYSTEM + to).to_le_bytes();
    d[16] = 6;
    put(&mut d, 0x70, &pointer(0xB0));
    d[0x98] = 1;
    put(&mut d, 0xB0, &pointer(0x100));
    let at = 0x100;
    d[at + 16] = 5;
    put(&mut d, at + 0x88, &pointer(0x300));
    for k in 0..3 {
        put(&mut d, at + 0x90 + k * 4, &0.5f32.to_le_bytes());
        put(&mut d, at + 0xA0 + k * 4, &10f32.to_le_bytes());
    }
    put(&mut d, at + 0xB0, &pointer(0x340));
    d[at + 0xD0] = 3;
    d[at + 0xD4] = 2;
    put(&mut d, at + 0xF0, &pointer(0x360));
    put(&mut d, at + 0x118, &pointer(0x370));
    d[at + 0x120] = 1;
    put(&mut d, 0x300, &[0xff, 0, 1, 0, 0, 0, 1, 0, 2, 0]);
    put(&mut d, 0x310, &[0, 0, 0, 2]);
    put(&mut d, 0x314, &1f32.to_le_bytes());
    put(&mut d, 0x318, &[1, 0]);
    put(&mut d, 0x340, &[0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0xfe, 0xff, 0, 0]);
    put(&mut d, 0x360, &[0xef, 0x5d, 0, 0]);
    d
}

#[test]
fn reads_triangles_and_capsules() {
    let (d, stats) = parse(&collision()).unwrap();
    let expected = Stats { bounds: 2, triangles: 1, primitives: 1, unknown: 0, materials: vec![0x5def] };
    assert_eq!(stats, expected);
    assert_eq!(d.meshes[0].positions[..3], [[10.0, 10.0, 10.0], [12.0, 10.0, 10.0], [10.0, 9.0, 10.0]]);
    assert_eq!(d.shaders[0].name_hash, MATERIAL_BASE + 0x5def);
}

#[test]
fn damaged_collisions_are_errors() {
    assert!(parse(&collision()[..0x345]).is_err());
    assert!(parse(&[0; 64]).is_err());
    let mut d = collision();
    d[0x304] = 9;
    assert!(parse(&d).is_err());
    // A composite that contains itself.
    let mut d = collision();
    d[0xB0..0xB8].copy_from_slice(&SYSTEM.to_le_bytes());
    assert!(parse(&d).is_err());
}

fn put(d: &mut [u8], at: usize, bytes: &[u8]) {
    d[at..at + bytes.len()].copy_from_slice(bytes);
}

fn put_ptr(d: &mut [u8], at: usize, to: usize) {
    put(d, at, &(SYSTEM + to as u64).to_le_bytes());
}

fn error_text(r: Result<(Drawable, Stats), BnError>) -> String {
    r.unwrap_err().to_string()
}

fn extent(d: &Drawable, lo: [f32; 3], hi: [f32; 3]) -> bool {
    let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3);
    near(d.bounds_min, lo) && near(d.bounds_max, hi)
}

/// A geometry bound (kind 5) at `at` with quantum 1, centre 0, integer vertices, 16-byte polygons,
/// material kinds and the material number of each polygon. Data goes behind `at + 0x140`.
fn geometry(d: &mut [u8], at: usize, verts: &[[i16; 3]], polys: &[[u8; 16]], kinds: &[u16], numbers: &[u8]) {
    d[at + 16] = 5;
    let base = at + 0x140;
    put_ptr(d, at + 0x88, base);
    for k in 0..3 {
        put(d, at + 0x90 + k * 4, &1.0f32.to_le_bytes());
    }
    let (verts_at, kinds_at) = (base + polys.len() * 16 + 0x10, base + polys.len() * 16 + 0x40);
    let numbers_at = kinds_at + kinds.len() * 8 + 0x10;
    put_ptr(d, at + 0xB0, verts_at);
    put(d, at + 0xD0, &(verts.len() as u32).to_le_bytes());
    put(d, at + 0xD4, &(polys.len() as u32).to_le_bytes());
    put_ptr(d, at + 0xF0, kinds_at);
    put_ptr(d, at + 0x118, numbers_at);
    d[at + 0x120] = kinds.len() as u8;
    for (i, p) in polys.iter().enumerate() {
        put(d, base + i * 16, p);
    }
    for (i, v) in verts.iter().enumerate() {
        for (k, c) in v.iter().enumerate() {
            put(d, verts_at + i * 6 + k * 2, &c.to_le_bytes());
        }
    }
    for (i, kind) in kinds.iter().enumerate() {
        put(d, kinds_at + i * 8, &kind.to_le_bytes());
    }
    put(d, numbers_at, numbers);
}

fn triangle(a: u16, b: u16, c: u16) -> [u8; 16] {
    let mut p = [0u8; 16];
    for (k, v) in [a, b, c].iter().enumerate() {
        put(&mut p, 4 + k * 2, &v.to_le_bytes());
    }
    p
}

fn round(polygon_kind: u8, a: u16, b: u16, radius: f32) -> [u8; 16] {
    let mut p = [0u8; 16];
    put(&mut p, 0, &a.to_le_bytes());
    p[3] = polygon_kind;
    put(&mut p, 4, &radius.to_le_bytes());
    put(&mut p, 8, &b.to_le_bytes());
    p
}

const TRI_VERTS: [[i16; 3]; 3] = [[0, 0, 0], [4, 0, 0], [0, 4, 0]];

fn single(verts: &[[i16; 3]], polys: &[[u8; 16]], kinds: &[u16], numbers: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x1000];
    geometry(&mut d, 0, verts, polys, kinds, numbers);
    d
}

#[test]
fn a_file_without_a_bound_is_refused() {
    assert_eq!(error_text(parse(&[])), "damaged or unknown collision data: no collision");
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    d[16] = 9;
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: no collision");
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    d[0] = 1;
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: no collision");
}

#[test]
fn triangles_are_grouped_by_material_type() {
    let polys = [triangle(0, 1, 2), triangle(0, 2, 1), triangle(1, 0, 2)];
    let (d, stats) = parse(&single(&TRI_VERTS, &polys, &[0x30, 0x40], &[1, 0, 1])).unwrap();
    assert_eq!(stats, Stats { bounds: 1, triangles: 3, primitives: 0, unknown: 0, materials: vec![0x40, 0x30] });
    assert_eq!(d.meshes.len(), 2);
    assert_eq!(d.meshes[0].indices.len(), 6);
    assert_eq!(d.shaders[0].name_hash, MATERIAL_BASE + 0x40);
    assert_eq!(d.shaders[1].name_hash, MATERIAL_BASE + 0x30);
    assert!(extent(&d, [0.0; 3], [4.0, 4.0, 0.0]));
}

#[test]
fn a_material_number_beyond_the_list_gives_type_zero() {
    let (_, stats) = parse(&single(&TRI_VERTS, &[triangle(0, 1, 2)], &[0x30], &[5])).unwrap();
    assert_eq!(stats.materials, [0]);
}

#[test]
fn polygons_without_material_tables_give_type_zero() {
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[0x30], &[0]);
    d[0xF0..0xF8].fill(0);
    d[0x118..0x120].fill(0);
    let (_, stats) = parse(&d).unwrap();
    assert_eq!((stats.triangles, stats.materials), (1, vec![0]));
}

#[test]
fn vertices_use_quantum_and_centre() {
    let mut d = single(&[[0, 0, 0], [10, 0, 0], [0, 10, 0]], &[triangle(0, 1, 2)], &[1], &[0]);
    for (k, q) in [0.5f32, 0.25, 1.0].iter().enumerate() {
        put(&mut d, 0x90 + k * 4, &q.to_le_bytes());
        put(&mut d, 0xA0 + k * 4, &(100.0 * (k as f32 + 1.0)).to_le_bytes());
    }
    let (dr, _) = parse(&d).unwrap();
    assert_eq!(dr.meshes[0].positions, [[100.0, 200.0, 300.0], [105.0, 200.0, 300.0], [100.0, 202.5, 300.0]]);
}

#[test]
fn capsules_and_cylinders_run_between_two_vertices() {
    let verts = [[0, 0, 0], [0, 0, 6]];
    let (d, stats) = parse(&single(&verts, &[round(2, 0, 1, 1.0)], &[1], &[0])).unwrap();
    assert_eq!((stats.primitives, stats.triangles), (1, 0));
    assert!(extent(&d, [-1.0, -1.0, -1.0], [1.0, 1.0, 7.0]));
    let (d, stats) = parse(&single(&verts, &[round(4, 0, 1, 1.0)], &[1], &[0])).unwrap();
    assert_eq!(stats.primitives, 1);
    assert!(extent(&d, [-1.0, -1.0, 0.0], [1.0, 1.0, 6.0]));
}

#[test]
fn a_triangle_naming_a_missing_vertex_is_refused() {
    let d = single(&TRI_VERTS, &[triangle(0, 1, 3)], &[1], &[0]);
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a triangle names no vertex");
}

#[test]
fn a_capsule_naming_a_missing_vertex_or_a_bad_radius_is_refused() {
    let message = "damaged or unknown collision data: a capsule names no vertex";
    assert_eq!(error_text(parse(&single(&TRI_VERTS, &[round(2, 0, 7, 1.0)], &[1], &[0]))), message);
    assert_eq!(error_text(parse(&single(&TRI_VERTS, &[round(4, 0, 1, f32::NAN)], &[1], &[0]))), message);
}

#[test]
fn polygons_of_other_kinds_are_counted_as_unknown() {
    let mut other = triangle(0, 1, 2);
    other[3] = 1;
    let (d, stats) = parse(&single(&TRI_VERTS, &[other, triangle(0, 1, 2)], &[1], &[0, 0])).unwrap();
    assert_eq!((stats.unknown, stats.triangles), (1, 1));
    assert_eq!(d.meshes.len(), 1);
}

#[test]
fn a_geometry_without_vertices_or_polygons_is_empty() {
    let (d, stats) = parse(&single(&[], &[], &[1], &[])).unwrap();
    assert_eq!(stats, Stats { bounds: 1, ..Default::default() });
    assert!(d.meshes.is_empty());
    assert_eq!((d.bounds_min, d.bounds_max), ([0.0; 3], [0.0; 3]));
}

#[test]
fn impossible_counts_are_refused() {
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    put(&mut d, 0xD0, &((1u32 << 21) + 1).to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: impossible vertex count");
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    put(&mut d, 0xD4, &((1u32 << 21) + 1).to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: impossible polygon count");
}

#[test]
fn damaged_vectors_and_pointers_are_refused() {
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    put(&mut d, 0x90, &f32::NAN.to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a vector is damaged");
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    put(&mut d, 0xB0, &0u64.to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a pointer leads nowhere");
    let mut d = single(&TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    put(&mut d, 0x88, &(SYSTEM + 0x10_0000).to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a pointer leads nowhere");
}

#[test]
fn a_cut_off_composite_is_refused() {
    let mut d = vec![0u8; 0x20];
    d[16] = 6;
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a bound is cut off");
    assert_eq!(error_text(parse(&[0u8; 16])), "damaged or unknown collision data: no collision");
}

fn composite(d: &mut [u8], at: usize, children: &[usize], transforms: Option<usize>) {
    d[at + 16] = 6;
    put_ptr(d, at + 0x70, 0x400);
    if let Some(t) = transforms {
        put_ptr(d, at + 0x78, t);
    }
    d[at + 0x98] = children.len() as u8;
    for (i, c) in children.iter().enumerate() {
        if *c != 0 {
            put_ptr(d, 0x400 + i * 8, *c);
        }
    }
}

fn translation(d: &mut [u8], at: usize, t: [f32; 3]) {
    for k in 0..3 {
        put(d, at + k * 20, &1.0f32.to_le_bytes());
    }
    for (k, v) in t.iter().enumerate() {
        put(d, at + 48 + k * 4, &v.to_le_bytes());
    }
    put(d, at + 12, &9.0f32.to_le_bytes());
}

#[test]
fn composite_children_are_moved_by_their_transforms() {
    let mut d = vec![0u8; 0x2000];
    composite(&mut d, 0, &[0x800, 0, 0x1000], Some(0x600));
    geometry(&mut d, 0x800, &TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    geometry(&mut d, 0x1000, &TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    translation(&mut d, 0x600, [10.0, 0.0, 0.0]);
    translation(&mut d, 0x600 + 128, [0.0, 0.0, 20.0]);
    let (dr, stats) = parse(&d).unwrap();
    assert_eq!((stats.bounds, stats.triangles, stats.materials), (3, 2, vec![1]));
    assert!(extent(&dr, [0.0, 0.0, 0.0], [14.0, 4.0, 20.0]));
}

#[test]
fn composite_children_without_transforms_stay_in_place() {
    let mut d = vec![0u8; 0x2000];
    composite(&mut d, 0, &[0x800], None);
    geometry(&mut d, 0x800, &TRI_VERTS, &[triangle(0, 1, 2)], &[1], &[0]);
    let (dr, _) = parse(&d).unwrap();
    assert!(extent(&dr, [0.0; 3], [4.0, 4.0, 0.0]));
}

#[test]
fn an_empty_composite_is_fine() {
    let mut d = vec![0u8; 0x2000];
    composite(&mut d, 0, &[], None);
    let (dr, stats) = parse(&d).unwrap();
    assert_eq!(stats, Stats { bounds: 1, ..Default::default() });
    assert!(dr.meshes.is_empty());
}

#[test]
fn children_of_unknown_kinds_are_counted() {
    let mut d = vec![0u8; 0x2000];
    composite(&mut d, 0, &[0x800], None);
    d[0x800 + 16] = 9;
    let (_, stats) = parse(&d).unwrap();
    assert_eq!((stats.bounds, stats.unknown), (2, 1));
}

#[test]
fn a_child_pointer_leading_nowhere_is_refused() {
    let mut d = vec![0u8; 0x2000];
    composite(&mut d, 0, &[0x800], None);
    put(&mut d, 0x400, &(SYSTEM + 0x10_0000).to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a pointer leads nowhere");
}

#[test]
fn a_composite_with_a_cut_off_child_is_refused() {
    let mut d = vec![0u8; 0x2000];
    composite(&mut d, 0, &[0x800], None);
    put(&mut d, 0x400, &(SYSTEM + 0x1FF0).to_le_bytes());
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: a pointer leads nowhere");
}

#[test]
fn too_many_bounds_are_refused() {
    let mut d = vec![0u8; 0x1000 + 65535 * 8];
    d[16] = 6;
    put_ptr(&mut d, 0x70, 0x400);
    put(&mut d, 0x98, &2u16.to_le_bytes());
    put_ptr(&mut d, 0x400, 0x100);
    put_ptr(&mut d, 0x408, 0x100);
    d[0x100 + 16] = 6;
    put_ptr(&mut d, 0x100 + 0x70, 0x1000);
    put(&mut d, 0x100 + 0x98, &65535u16.to_le_bytes());
    for i in 0..65535 {
        put_ptr(&mut d, 0x1000 + i * 8, 0x200);
    }
    d[0x200 + 16] = 9;
    assert_eq!(error_text(parse(&d)), "damaged or unknown collision data: bounds nested too deeply");
}

#[test]
fn material_types_beyond_256_share_the_last_number() {
    let mut d = vec![0u8; 0x8000];
    composite(&mut d, 0, &[0x800, 0x4000], None);
    let kinds: Vec<u16> = (0..255).collect();
    let numbers: Vec<u8> = (0..255).collect();
    let polys: Vec<[u8; 16]> = (0..255).map(|_| triangle(0, 1, 2)).collect();
    geometry(&mut d, 0x800, &TRI_VERTS, &polys, &kinds, &numbers);
    geometry(&mut d, 0x4000, &TRI_VERTS, &[triangle(0, 1, 2), triangle(0, 1, 2)], &[1000, 1001], &[0, 1]);
    let (dr, stats) = parse(&d).unwrap();
    assert_eq!(stats.materials.len(), 256);
    assert_eq!(stats.triangles, 257);
    assert_eq!(dr.meshes.len(), 256);
    assert_eq!(dr.meshes[255].indices.len(), 6, "the 257th type is drawn with the 256th");
    assert_eq!(dr.shaders[255].name_hash, MATERIAL_BASE + 1000);
}
