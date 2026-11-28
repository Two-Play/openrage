use super::*;
use crate::ybn;

const VERSION: u32 = 159;

fn finish(b: Builder) -> Resource {
    Resource::from_rsc7(&b.build(VERSION).unwrap()).unwrap()
}

fn read(file: &[u8]) -> Resource {
    Resource::from_rsc7(file).unwrap()
}

/// A BVH bound (type 8) with an empty hierarchy and one material of type `material`.
fn bvh_bound(b: &mut Builder, material: u8) -> BlockId {
    let mut bound = vec![0u8; BVH_BOUND_SIZE];
    bound[0x10] = kind::BVH;
    let bound = b.add(Segment::System, bound);
    let materials = b.add(Segment::System, vec![material, 0, 0, 0, 0, 0, 0, 0]);
    let bvh = b.add(Segment::System, vec![0u8; BVH_SIZE]);
    b.pointer(bound, 0xF0, materials, 0);
    b.pointer(bound, 0x130, bvh, 0);
    bound
}

fn plain_bound(b: &mut Builder, bound_kind: u8) -> BlockId {
    let mut bound = vec![0u8; BVH_BOUND_SIZE];
    bound[0x10] = bound_kind;
    b.add(Segment::System, bound)
}

fn composite_root(b: &mut Builder) -> BlockId {
    let mut root = vec![0u8; 0xB0];
    root[0x10] = kind::COMPOSITE;
    b.add(Segment::System, root)
}

fn link_children(b: &mut Builder, composite: BlockId, children: &[BlockId]) {
    let list = b.add(Segment::System, vec![0u8; children.len() * 8]);
    for (i, child) in children.iter().enumerate() {
        b.pointer(list, i * 8, *child, 0);
    }
    put_u16(b.data_mut(composite), 0xA0, children.len() as u16);
    b.pointer(composite, 0x70, list, 0);
}

/// A `.ybn` whose composite root holds one BVH bound whose first material is `material`.
fn composite_with_bvh(material: u8) -> Resource {
    let mut b = Builder::new();
    let root = composite_root(&mut b);
    let child = bvh_bound(&mut b, material);
    link_children(&mut b, root, &[child]);
    finish(b)
}

/// A `.ybn` whose composite root holds only a box (type 3).
fn composite_with_box() -> Resource {
    let mut b = Builder::new();
    let root = composite_root(&mut b);
    let child = plain_bound(&mut b, kind::BOX);
    link_children(&mut b, root, &[child]);
    finish(b)
}

/// A `.ydr` with `bound` as its collision, or without one.
fn drawable_with(bound: Option<u8>) -> Resource {
    let mut b = Builder::new();
    let root = b.add(Segment::System, vec![0u8; 0xD0]);
    if let Some(material) = bound {
        let bound = bvh_bound(&mut b, material);
        b.pointer(root, 0xC8, bound, 0);
    }
    finish(b)
}

/// `n` composites, the last one holding a BVH bound.
fn nested(n: usize) -> Resource {
    let mut b = Builder::new();
    let composites: Vec<BlockId> = (0..n).map(|_| composite_root(&mut b)).collect();
    let bound = bvh_bound(&mut b, 0);
    for (i, c) in composites.iter().enumerate() {
        match composites.get(i + 1) {
            Some(next) => link_children(&mut b, *c, &[*next]),
            None => link_children(&mut b, *c, &[bound]),
        }
    }
    finish(b)
}

fn triangle() -> CollisionMesh {
    CollisionMesh {
        positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
        indices: vec![0, 1, 2],
        ..Default::default()
    }
}

fn quad() -> CollisionMesh {
    CollisionMesh {
        positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Default::default()
    }
}

/// `n` × `n` squares (two triangles each) on a slightly wavy plane.
fn grid(n: usize) -> CollisionMesh {
    let mut mesh = CollisionMesh::default();
    for y in 0..=n {
        for x in 0..=n {
            mesh.positions.push([x as f32, y as f32, ((x * y) % 7) as f32 * 0.1]);
        }
    }
    let w = n as u32 + 1;
    for y in 0..n {
        for x in 0..n {
            let i = y as u32 * w + x as u32;
            mesh.indices.extend([i, i + 1, i + w + 1, i, i + w + 1, i + w]);
            mesh.materials.extend([Some((x % 3) as u8), Some(9)]);
        }
    }
    mesh
}

fn err_text<T>(r: Result<T, ResourceError>) -> String {
    match r {
        Ok(_) => panic!("an error was expected"),
        Err(e) => e.to_string(),
    }
}

/// The new bound of a replaced `.ybn` (the composite's only child).
fn first_child(res: &Resource) -> u64 {
    res.u64(res.u64(SYS + 0x70).unwrap()).unwrap()
}

fn f32_of(res: &Resource, at: u64) -> f32 {
    f32::from_bits(res.u32(at).unwrap())
}

fn vec3_bytes(v: [f32; 3]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn builds_hierarchies_the_game_way() {
    let mesh = grid(40);
    let g = build_geometry(&mesh, 0).unwrap();
    assert_eq!(g.polygons.len(), 3200);
    assert_eq!(g.vertices.len(), 41 * 41, "shared corners once");
    assert_eq!(g.materials.len(), 4);
    let inner = g.polygons.iter().filter(|p| p.2.iter().all(|&e| e >= 0)).count();
    assert!(inner > 3000, "{inner}");
    for (i, p) in g.polygons.iter().enumerate() {
        for k in 0..3 {
            if p.2[k] >= 0 {
                let q = &g.polygons[p.2[k] as usize];
                assert!(q.0.contains(&p.0[k]) && q.0.contains(&p.0[(k + 1) % 3]), "edge {k} of {i}");
            }
        }
    }
    let mut next = 0;
    for n in &g.nodes {
        if n.count > 0 {
            assert_eq!(n.item, next);
            assert!(n.count <= LEAF_ITEMS);
            next += n.count;
        }
    }
    assert_eq!(next, 3200);
    assert_eq!(g.nodes[0].item, g.nodes.len());
    assert!(g.subtrees.iter().all(|&(a, b)| b - a <= SUBTREE_NODES));
    assert!(!g.subtrees.is_empty());
    let too_many =
        CollisionMesh { positions: vec![[0.0; 3]; 3], indices: [0, 1, 2].repeat(40000), ..Default::default() };
    assert!(build_geometry(&too_many, 0).is_err());
}

#[test]
fn a_drawable_keeps_its_collision_at_0xc8() {
    let res = drawable_with(Some(0));
    let at = collision_of(&res, true).unwrap();
    assert_eq!(at, res.u64(SYS + 0xC8).unwrap());
    assert_eq!(res.u8(at + 0x10).unwrap(), kind::BVH);
}

#[test]
fn a_ybn_keeps_its_collision_at_the_root() {
    assert_eq!(collision_of(&composite_with_box(), false).unwrap(), SYS);
}

#[test]
fn a_drawable_without_collision_is_an_error() {
    let res = drawable_with(None);
    assert_eq!(err_text(collision_of(&res, true)), "the drawable has no collision");
}

#[test]
fn a_collision_pointer_into_graphics_memory_is_no_collision() {
    let mut res = drawable_with(Some(0));
    res.bytes_mut(SYS + 0xC8, 8).unwrap().copy_from_slice(&GFX.to_le_bytes());
    assert_eq!(err_text(collision_of(&res, true)), "the drawable has no collision");
}

#[test]
fn only_resources_with_a_bvh_bound_can_donate() {
    assert!(can_donate(&composite_with_bvh(0), false));
    assert!(can_donate(&drawable_with(Some(0)), true));
    assert!(!can_donate(&composite_with_box(), false));
    assert!(!can_donate(&drawable_with(None), true));
    assert!(!can_donate(&composite_with_bvh(0), true), "a ybn read as a drawable has no collision pointer");
}

#[test]
fn bvh_bounds_are_found_below_nested_composites() {
    let res = nested(4);
    assert!(can_donate(&res, false));
    assert_eq!(bvh_bounds(&res, SYS).len(), 1);
    let res = nested(5);
    assert!(!can_donate(&res, false), "composites nest at most four levels deep");
    assert!(bvh_bounds(&res, SYS).is_empty());
}

#[test]
fn bvh_bounds_skip_children_that_are_no_system_pointers() {
    let mut b = Builder::new();
    let root = composite_root(&mut b);
    let first = bvh_bound(&mut b, 0);
    let second = bvh_bound(&mut b, 0);
    link_children(&mut b, root, &[first, second]);
    let mut res = finish(b);
    let list = res.u64(SYS + 0x70).unwrap();
    assert_eq!(bvh_bounds(&res, SYS).len(), 2);
    res.bytes_mut(list + 8, 8).unwrap().copy_from_slice(&0x1234u64.to_le_bytes());
    assert_eq!(bvh_bounds(&res, SYS), [res.u64(list).unwrap()]);
    assert!(can_donate(&res, false));
}

#[test]
fn a_bound_of_another_kind_has_no_bvh() {
    assert!(bvh_bounds(&composite_with_box(), SYS).is_empty());
    assert!(bvh_bounds(&composite_with_box(), 0x1000).is_empty(), "an unreadable address finds nothing");
}

#[test]
fn a_mesh_without_triangles_is_refused() {
    let none = CollisionMesh { positions: vec![[0.0; 3]; 3], ..Default::default() };
    assert_eq!(err_text(check_mesh(&none)), "the collision has no triangles");
    let partial = CollisionMesh { positions: vec![[0.0; 3]; 4], indices: vec![0, 1, 2, 3], ..Default::default() };
    assert_eq!(err_text(check_mesh(&partial)), "the collision has no triangles");
}

#[test]
fn a_triangle_pointing_at_a_missing_vertex_is_refused() {
    let mesh = CollisionMesh { positions: vec![[0.0; 3]; 3], indices: vec![0, 1, 3], ..Default::default() };
    assert_eq!(err_text(check_mesh(&mesh)), "the collision has triangles pointing at missing vertices");
}

#[test]
fn a_material_list_of_the_wrong_length_is_refused() {
    let mesh = CollisionMesh { materials: vec![None, None], ..triangle() };
    assert_eq!(err_text(check_mesh(&mesh)), "the collision has a material list of the wrong length");
}

#[test]
fn positions_that_are_not_numbers_are_refused() {
    for bad_value in [f32::NAN, f32::INFINITY] {
        let mut mesh = triangle();
        mesh.positions[1][2] = bad_value;
        assert_eq!(err_text(check_mesh(&mesh)), "the collision has positions that are not numbers");
    }
}

#[test]
fn a_valid_mesh_passes_the_check() {
    check_mesh(&triangle()).unwrap();
    check_mesh(&CollisionMesh { materials: vec![Some(1)], ..triangle() }).unwrap();
}

#[test]
fn a_triangle_is_quantized_around_its_centre() {
    let g = build_geometry(&triangle(), 0).unwrap();
    assert_eq!(g.min, [0.0, 0.0, 0.0]);
    assert_eq!(g.max, [2.0, 2.0, 0.0]);
    assert_eq!(g.centre, [1.0, 1.0, 0.0]);
    assert_eq!(g.quantum, [2.0 / 65534.0, 2.0 / 65534.0, 1e-6]);
    assert_eq!(g.vertices, [[-32767, -32767, 0], [32767, -32767, 0], [-32767, 32767, 0]]);
    assert_eq!(g.polygons.len(), 1);
    let (corners, area, neighbours, material) = g.polygons[0];
    assert_eq!(corners, [0, 1, 2]);
    assert!((area - 2.0).abs() < 1e-3, "{area}");
    assert_eq!(neighbours, [-1, -1, -1]);
    assert_eq!(material, 0);
    assert_eq!(g.nodes.len(), 1);
    assert_eq!((g.nodes[0].item, g.nodes[0].count), (0, 1));
    assert_eq!(g.subtrees, [(0, 1)]);
}

#[test]
fn neighbours_are_the_polygons_across_an_edge() {
    let g = build_geometry(&quad(), 0).unwrap();
    assert_eq!(g.vertices.len(), 4);
    assert_eq!(g.polygons[0].0, [0, 1, 2]);
    assert_eq!(g.polygons[1].0, [0, 2, 3]);
    assert_eq!(g.polygons[0].2, [-1, -1, 1]);
    assert_eq!(g.polygons[1].2, [0, -1, -1]);
}

#[test]
fn equal_places_share_one_vertex() {
    let mesh = CollisionMesh {
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
        indices: vec![0, 1, 2, 3, 4, 5],
        ..Default::default()
    };
    let g = build_geometry(&mesh, 0).unwrap();
    assert_eq!(g.vertices.len(), 4);
    assert_eq!(g.polygons[1].0, [1, 2, 3]);
    assert_eq!(g.polygons[0].2[1], 1, "the edge between the two triangles");
}

#[test]
fn materials_without_a_type_get_the_default() {
    let mesh = CollisionMesh { materials: vec![Some(7), None, Some(7)], ..quad_pair() };
    let g = build_geometry(&mesh, 5).unwrap();
    assert_eq!(g.materials, [7, 5]);
    assert_eq!(g.polygons.iter().map(|p| p.3).collect::<Vec<_>>(), [0, 1, 0]);
    let plain = build_geometry(&quad(), 9).unwrap();
    assert_eq!(plain.materials, [9]);
}

fn quad_pair() -> CollisionMesh {
    CollisionMesh {
        positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0]],
        indices: vec![0, 1, 2, 0, 2, 3, 1, 2, 3],
        ..Default::default()
    }
}

#[test]
fn up_to_255_materials_are_kept() {
    let mut mesh = CollisionMesh { positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], ..Default::default() };
    for m in 0..255u8 {
        mesh.indices.extend([0, 1, 2]);
        mesh.materials.push(Some(m));
    }
    let g = build_geometry(&mesh, 0).unwrap();
    assert_eq!(g.materials.len(), 255);
    assert_eq!(g.polygons.len(), 255);
}

#[test]
fn too_many_triangles_are_refused() {
    let mesh = CollisionMesh { positions: vec![[0.0; 3]; 3], indices: [0, 1, 2].repeat(40000), ..Default::default() };
    assert_eq!(err_text(build_geometry(&mesh, 0)), "the collision has 40000 triangles; at most 32767");
}

#[test]
fn too_many_vertices_are_refused() {
    let mut mesh = CollisionMesh::default();
    for t in 0..11000u32 {
        for k in 0..3 {
            mesh.positions.push([(t * 3 + k) as f32, 0.0, 0.0]);
        }
        mesh.indices.extend([t * 3, t * 3 + 1, t * 3 + 2]);
    }
    assert_eq!(err_text(build_geometry(&mesh, 0)), "the collision has more than 32767 vertices");
}

#[test]
fn the_bound_box_holds_the_mesh_and_a_sphere_around_it() {
    let g = build_geometry(&triangle(), 0).unwrap();
    let mut d = vec![0u8; BVH_BOUND_SIZE];
    put_bound_box(&mut d, 0, &g);
    let f = |at: usize| f32::from_le_bytes(d[at..at + 4].try_into().unwrap());
    assert_eq!([f(0x20), f(0x24), f(0x28)], [2.0, 2.0, 0.0], "box max");
    assert_eq!([f(0x30), f(0x34), f(0x38)], [0.0, 0.0, 0.0], "box min");
    assert_eq!([f(0x40), f(0x44), f(0x48)], [1.0, 1.0, 0.0], "box centre");
    assert_eq!([f(0x50), f(0x54), f(0x58)], [1.0, 1.0, 0.0], "sphere centre");
    assert!((f(0x14) - 2.0f32.sqrt()).abs() < 1e-6, "sphere radius");
}

#[test]
fn a_replaced_composite_holds_the_new_mesh() {
    let template = composite_with_bvh(5);
    let file = replace_collision(&template, false, &quad(), None).unwrap();
    let res = read(&file);
    let (d, stats) = ybn::parse(&res).unwrap();
    assert_eq!((stats.bounds, stats.triangles, stats.vertices), (2, 2, 4));
    assert_eq!(stats.materials, [5], "the template's first material is the default");
    assert_eq!(d.meshes.len(), 1);
    assert_eq!(d.meshes[0].indices.len(), 6);
    assert!(close(d.bounds_min, [0.0, 0.0, 0.0]) && close(d.bounds_max, [2.0, 2.0, 0.0]));
    let corners: Vec<[f32; 3]> = d.meshes[0].indices.iter().map(|&i| d.meshes[0].positions[i as usize]).collect();
    let want = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0]];
    assert!(corners.iter().zip(want).all(|(a, b)| close(*a, b)), "{corners:?}");
    let bound = first_child(&res);
    assert_eq!(res.u8(bound + 0x10).unwrap(), kind::BVH);
    assert_eq!(bvh_bounds(&res, SYS), [bound]);
    check_bvh(&res, bound).unwrap();
}

#[test]
fn the_new_bound_holds_polygons_vertices_and_materials() {
    let template = composite_with_bvh(5);
    let mesh = CollisionMesh { materials: vec![Some(3), None], ..quad() };
    let res = read(&replace_collision(&template, false, &mesh, None).unwrap());
    let bound = first_child(&res);
    assert_eq!(res.u32(bound + 0x84).unwrap(), 4);
    assert_eq!((res.u32(bound + 0xD0).unwrap(), res.u32(bound + 0xD4).unwrap()), (4, 2));
    assert_eq!(res.u8(bound + 0x120).unwrap(), 2);
    let materials = res.u64(bound + 0xF0).unwrap();
    assert_eq!([res.u8(materials).unwrap(), res.u8(materials + 8).unwrap()], [3, 5]);
    let per_polygon = res.u64(bound + 0x118).unwrap();
    assert_eq!(res.bytes(per_polygon, 2).unwrap(), [0, 1]);
    let polys = res.u64(bound + 0x88).unwrap();
    let first = res.bytes(polys, 16).unwrap();
    assert_eq!(first[0] & 7, 0, "a triangle");
    assert_eq!(&first[4..10], [0, 0, 1, 0, 2, 0]);
    assert_eq!(&first[10..16], [0xFF, 0xFF, 0xFF, 0xFF, 1, 0]);
    let area = f32::from_le_bytes(first[0..4].try_into().unwrap());
    assert!((area - 2.0).abs() < 1e-3, "{area}");
    assert_eq!(f32_of(&res, bound + 0x90), 2.0 / 65534.0);
    assert_eq!(f32_of(&res, bound + 0x98), 1e-6);
    assert!(close(
        [f32_of(&res, bound + 0xA0), f32_of(&res, bound + 0xA4), f32_of(&res, bound + 0xA8)],
        [1.0, 1.0, 0.0]
    ));
    let (_, stats) = ybn::parse(&res).unwrap();
    assert_eq!(stats.materials, [3, 5]);
}

#[test]
fn the_composite_keeps_one_child_with_the_boxes_of_the_mesh() {
    let mut b = Builder::new();
    let root = composite_root(&mut b);
    let child = bvh_bound(&mut b, 0);
    link_children(&mut b, root, &[child]);
    let transform = b.add(Segment::System, vec![0xAB; 64]);
    let aabb = b.add(Segment::System, vec![0xCD; 32]);
    let flags = b.add(Segment::System, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    b.pointer(root, 0x78, transform, 0);
    b.pointer(root, 0x88, aabb, 0);
    b.pointer(root, 0x90, flags, 0);
    b.data_mut(root)[0xA8..0xB0].fill(0xEE);
    b.add(Segment::Graphics, vec![7u8; 32]);
    let template = finish(b);
    let res = read(&replace_collision(&template, false, &quad(), None).unwrap());
    assert_eq!(res.graphics[..32], [7u8; 32], "graphics memory is kept");
    assert_eq!(res.u16(SYS + 0xA0).unwrap(), 1);
    assert_eq!(res.u16(SYS + 0xA2).unwrap(), 1);
    assert_eq!(res.u64(SYS + 0xA8).unwrap(), 0, "no hierarchy of its own");
    let transforms = res.u64(SYS + 0x78).unwrap();
    assert_eq!(res.u64(SYS + 0x80).unwrap(), transforms);
    let t = res.bytes(transforms, 64).unwrap();
    for (k, v) in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0].iter().enumerate() {
        let at = (k / 3) * 16 + (k % 3) * 4;
        assert_eq!(t[at..at + 4], v.to_le_bytes(), "element {k}");
    }
    for w in [12, 28, 44, 60] {
        assert_eq!(t[w..w + 4], [0xAB; 4], "the w of a column keeps the template's");
    }
    let aabb = res.bytes(res.u64(SYS + 0x88).unwrap(), 32).unwrap();
    assert_eq!(&aabb[0..12], vec3_bytes([0.0, 0.0, 0.0]));
    assert_eq!(&aabb[16..28], vec3_bytes([2.0, 2.0, 0.0]));
    assert_eq!(&aabb[12..16], [0xCD; 4]);
    let flags = res.u64(SYS + 0x90).unwrap();
    assert_eq!(res.u64(SYS + 0x98).unwrap(), flags);
    assert_eq!(res.bytes(flags, 16).unwrap(), [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(close([f32_of(&res, SYS + 0x20), f32_of(&res, SYS + 0x24), f32_of(&res, SYS + 0x28)], [2.0, 2.0, 0.0]));
}

#[test]
fn a_composite_without_transforms_still_gets_them() {
    let template = composite_with_bvh(0);
    assert_eq!(template.u64(SYS + 0x78).unwrap(), 0);
    let res = read(&replace_collision(&template, false, &triangle(), None).unwrap());
    let transforms = res.u64(SYS + 0x78).unwrap();
    assert_ne!(transforms, 0);
    let t = res.bytes(transforms, 64).unwrap();
    assert_eq!(f32::from_le_bytes(t[0..4].try_into().unwrap()), 1.0);
    assert_eq!(f32::from_le_bytes(t[20..24].try_into().unwrap()), 1.0);
    assert_eq!(f32::from_le_bytes(t[40..44].try_into().unwrap()), 1.0);
}

#[test]
fn a_single_bound_in_a_drawable_is_replaced() {
    let template = drawable_with(Some(4));
    let old = template.u64(SYS + 0xC8).unwrap();
    let res = read(&replace_collision(&template, true, &quad(), None).unwrap());
    let at = collision_of(&res, true).unwrap();
    assert_ne!(at, old);
    assert_eq!(res.u8(at + 0x10).unwrap(), kind::BVH);
    let (d, stats) = ybn::parse_at(&res, at).unwrap();
    assert_eq!((stats.bounds, stats.triangles, stats.vertices), (1, 2, 4));
    assert_eq!(stats.materials, [4]);
    assert_eq!(d.meshes.len(), 1);
    check_bvh(&res, at).unwrap();
}

#[test]
fn a_ybn_whose_root_is_no_composite_cannot_get_a_bound() {
    let mut b = Builder::new();
    bvh_bound(&mut b, 0);
    let template = finish(b);
    let message = err_text(replace_collision(&template, false, &triangle(), None));
    assert_eq!(message, "the collision file's root is no composite: it cannot get a new bound");
}

#[test]
fn a_drawable_without_collision_cannot_be_replaced() {
    let message = err_text(replace_collision(&drawable_with(None), true, &triangle(), None));
    assert_eq!(message, "the drawable has no collision");
}

#[test]
fn a_bad_mesh_is_refused_before_anything_is_built() {
    let message = err_text(replace_collision(&composite_with_bvh(0), false, &CollisionMesh::default(), None));
    assert_eq!(message, "the collision has no triangles");
}

#[test]
fn a_template_without_a_mesh_needs_a_donor() {
    let template = composite_with_box();
    let message = err_text(replace_collision(&template, false, &triangle(), None));
    assert_eq!(message, "the original collision has no triangle mesh to copy the structure from (give a donor)");
}

#[test]
fn a_donor_without_a_mesh_is_refused() {
    let template = composite_with_box();
    let donor = composite_with_box();
    let message = err_text(replace_collision(&template, false, &triangle(), Some((&donor, false))));
    assert_eq!(message, "the donor has no triangle mesh collision");
    let ydr = drawable_with(None);
    let message = err_text(replace_collision(&template, false, &triangle(), Some((&ydr, true))));
    assert_eq!(message, "the donor has no triangle mesh collision");
}

#[test]
fn a_donor_lends_its_structure() {
    let template = composite_with_box();
    let donor = composite_with_bvh(11);
    let res = read(&replace_collision(&template, false, &quad(), Some((&donor, false))).unwrap());
    let (_, stats) = ybn::parse(&res).unwrap();
    assert_eq!((stats.bounds, stats.triangles, stats.materials), (2, 2, vec![11]));
    check_bvh(&res, first_child(&res)).unwrap();
    let ydr_donor = drawable_with(Some(12));
    let res = read(&replace_collision(&template, false, &quad(), Some((&ydr_donor, true))).unwrap());
    assert_eq!(ybn::parse(&res).unwrap().1.materials, [12]);
}

#[test]
fn the_template_is_preferred_to_the_donor() {
    let template = composite_with_bvh(2);
    let donor = composite_with_bvh(11);
    let res = read(&replace_collision(&template, false, &triangle(), Some((&donor, false))).unwrap());
    assert_eq!(ybn::parse(&res).unwrap().1.materials, [2]);
}

fn written(mesh: &CollisionMesh) -> (Resource, u64) {
    let res = read(&replace_collision(&composite_with_bvh(0), false, mesh, None).unwrap());
    let at = first_child(&res);
    (res, at)
}

fn nodes_at(res: &Resource, at: u64) -> u64 {
    res.u64(res.u64(at + 0x130).unwrap()).unwrap()
}

fn node_count(res: &Resource, at: u64) -> usize {
    res.u32(res.u64(at + 0x130).unwrap() + 8).unwrap() as usize
}

/// The index of the first leaf and its first polygon.
fn first_leaf(res: &Resource, at: u64) -> (usize, usize) {
    let nodes = nodes_at(res, at);
    let i = (0..node_count(res, at)).find(|&i| res.u16(nodes + i as u64 * 16 + 14).unwrap() > 0).unwrap();
    (i, res.u16(nodes + i as u64 * 16 + 12).unwrap() as usize)
}

fn patch(res: &mut Resource, at: u64, bytes: &[u8]) {
    res.bytes_mut(at, bytes.len()).unwrap().copy_from_slice(bytes);
}

#[test]
fn written_hierarchies_check_out() {
    for n in [1, 2, 3, 6] {
        let (res, at) = written(&grid(n));
        check_bvh(&res, at).unwrap_or_else(|e| panic!("grid {n}: {e}"));
    }
    let (res, at) = written(&triangle());
    check_bvh(&res, at).unwrap();
    assert_eq!(node_count(&res, at), 1);
}

#[test]
fn a_large_written_hierarchy_has_subtree_headers() {
    let (res, at) = written(&grid(40));
    check_bvh(&res, at).unwrap();
    let bvh = res.u64(at + 0x130).unwrap();
    assert!(res.u16(bvh + 0x78).unwrap() > 1);
    assert_eq!(res.u16(bvh + 0x7A).unwrap(), res.u16(bvh + 0x78).unwrap());
}

#[test]
fn a_changed_subtree_header_is_found() {
    let (mut res, at) = written(&grid(40));
    let bvh = res.u64(at + 0x130).unwrap();
    let trees = res.u64(bvh + 0x70).unwrap();
    let end = res.u16(trees + 14).unwrap();
    patch(&mut res, trees + 14, &(end + 1).to_le_bytes());
    let count = res.u16(bvh + 0x78).unwrap();
    assert_eq!(err_text(check_bvh(&res, at)), format!("{count} subtree headers, expected {count}"));
}

#[test]
fn a_missing_subtree_header_is_found() {
    let (mut res, at) = written(&grid(3));
    let bvh = res.u64(at + 0x130).unwrap();
    patch(&mut res, bvh + 0x78, &0u16.to_le_bytes());
    assert_eq!(err_text(check_bvh(&res, at)), "0 subtree headers, expected 1");
}

#[test]
fn a_polygon_count_that_leaves_do_not_cover_is_found() {
    let (mut res, at) = written(&grid(3));
    patch(&mut res, at + 0xD4, &17u32.to_le_bytes());
    assert_eq!(err_text(check_bvh(&res, at)), "leaves cover 18 of 17 polygons");
}

#[test]
fn a_root_of_the_wrong_size_is_found() {
    let (mut res, at) = written(&grid(3));
    let nodes = nodes_at(&res, at);
    let count = node_count(&res, at);
    patch(&mut res, nodes + 12, &5u16.to_le_bytes());
    assert_eq!(err_text(check_bvh(&res, at)), format!("node 0: subtree of 5 nodes, its place ends at {count}"));
}

#[test]
fn a_leaf_of_more_than_four_polygons_is_found() {
    let (mut res, at) = written(&grid(3));
    let (leaf, first) = first_leaf(&res, at);
    let nodes = nodes_at(&res, at);
    patch(&mut res, nodes + leaf as u64 * 16 + 14, &5u16.to_le_bytes());
    let message = format!("leaf {leaf}: polygons {first}+5, expected 0, in a subtree to {}", leaf + 1);
    assert_eq!(err_text(check_bvh(&res, at)), message);
}

#[test]
fn a_leaf_out_of_polygon_order_is_found() {
    let (mut res, at) = written(&grid(3));
    let (leaf, _) = first_leaf(&res, at);
    let nodes = nodes_at(&res, at);
    let n = res.u16(nodes + leaf as u64 * 16 + 14).unwrap();
    patch(&mut res, nodes + leaf as u64 * 16 + 12, &1u16.to_le_bytes());
    let message = format!("leaf {leaf}: polygons 1+{n}, expected 0, in a subtree to {}", leaf + 1);
    assert_eq!(err_text(check_bvh(&res, at)), message);
}

#[test]
fn a_leaf_box_away_from_its_polygons_is_found() {
    let (mut res, at) = written(&grid(3));
    let (leaf, first) = first_leaf(&res, at);
    let corner: Vec<u8> = [-32767i16; 6].iter().flat_map(|v| v.to_le_bytes()).collect();
    let nodes = nodes_at(&res, at);
    patch(&mut res, nodes + leaf as u64 * 16, &corner);
    assert_eq!(err_text(check_bvh(&res, at)), format!("polygon {first} lies outside leaf {leaf}"));
}

#[test]
fn boxes_and_spheres_may_reach_out_of_their_leaf() {
    let (mut res, at) = written(&triangle());
    let corner: Vec<u8> = [-32767i16; 6].iter().flat_map(|v| v.to_le_bytes()).collect();
    let nodes = nodes_at(&res, at);
    patch(&mut res, nodes, &corner);
    assert!(check_bvh(&res, at).is_err());
    let polys = res.u64(at + 0x88).unwrap();
    let first = res.u8(polys).unwrap();
    patch(&mut res, polys, &[first | 3]);
    check_bvh(&res, at).unwrap();
}

#[test]
fn a_bound_without_a_hierarchy_is_unreadable() {
    let res = composite_with_bvh(0);
    let at = first_child(&res);
    assert!(check_bvh(&res, at).is_err());
}

fn raw_nodes(nodes: &[(u16, u16)]) -> Vec<u8> {
    nodes
        .iter()
        .flat_map(|&(item, count)| [[0u8; 12].as_slice(), &item.to_le_bytes(), &count.to_le_bytes()].concat())
        .collect()
}

fn structure(nodes: &[(u16, u16)], polys: usize) -> Result<(), ResourceError> {
    let res = Resource { version: 0, system: Vec::new(), graphics: Vec::new() };
    let raw = raw_nodes(nodes);
    let view = BvhView { res: &res, at: 0, bvh: 0, raw: &raw, count: nodes.len(), centre: [0.0; 3], quantum: [1.0; 3] };
    view.check_structure(polys)
}

#[test]
fn a_consistent_tree_passes_the_structure_check() {
    structure(&[(3, 0), (0, 2), (2, 3)], 5).unwrap();
    structure(&[(0, 4)], 4).unwrap();
}

#[test]
fn leaves_that_miss_polygons_are_found() {
    assert_eq!(err_text(structure(&[(3, 0), (0, 2), (2, 3)], 6)), "leaves cover 5 of 6 polygons");
}

#[test]
fn an_inner_node_of_fewer_than_three_nodes_is_found() {
    assert_eq!(err_text(structure(&[(2, 0), (0, 1)], 1)), "node 0: subtree of 2 nodes, its place ends at 2");
}

#[test]
fn a_leaf_of_five_polygons_is_found() {
    assert_eq!(err_text(structure(&[(0, 5)], 5)), "leaf 0: polygons 0+5, expected 0, in a subtree to 1");
}

#[test]
fn a_leaf_that_skips_polygons_is_found() {
    assert_eq!(
        err_text(structure(&[(3, 0), (0, 2), (3, 3)], 6)),
        "leaf 2: polygons 3+3, expected 2, in a subtree to 3"
    );
}

#[test]
fn a_leaf_in_a_larger_subtree_is_found() {
    assert_eq!(
        err_text(structure(&[(4, 0), (0, 2), (2, 3), (0, 0)], 5)),
        "leaf 2: polygons 2+3, expected 2, in a subtree to 4"
    );
}

#[test]
fn a_node_outside_its_parent_is_found() {
    let res = Resource { version: 0, system: Vec::new(), graphics: Vec::new() };
    let raw = raw_nodes(&[(3, 0), (0, 2), (2, 3)]);
    let view = BvhView { res: &res, at: 0, bvh: 0, raw: &raw, count: 3, centre: [0.0; 3], quantum: [1.0; 3] };
    let mut next = 0;
    assert_eq!(err_text(view.visit(3, 10, &mut next)), "node 3 lies outside its parent");
    assert_eq!(err_text(view.visit(2, 2, &mut next)), "node 2 lies outside its parent");
}

#[test]
fn visiting_gives_the_size_of_the_left_subtree() {
    let res = Resource { version: 0, system: Vec::new(), graphics: Vec::new() };
    let raw = raw_nodes(&[(5, 0), (3, 0), (0, 1), (1, 1), (0, 1)]);
    let view = BvhView { res: &res, at: 0, bvh: 0, raw: &raw, count: 5, centre: [0.0; 3], quantum: [1.0; 3] };
    let mut next = 0;
    assert_eq!(view.visit(0, 5, &mut next).unwrap(), Some(3), "the left child is an inner node of three");
    assert_eq!(view.visit(1, 4, &mut next).unwrap(), Some(1), "the left child is a leaf");
    assert_eq!(view.visit(2, 3, &mut next).unwrap(), None);
    assert_eq!(next, 1);
}
