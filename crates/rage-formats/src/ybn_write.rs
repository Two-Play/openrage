//! Writing collisions of GTA V Enhanced: a triangle mesh as a bound with a
//! bounding volume hierarchy (`phBoundBVH`, type 8), in place of the
//! collision of a `.ybn` or of a drawable (`.ydr`, at `0xC8`).
//!
//! Like the model writer, nothing of the structures is invented: the new
//! bound is a copy of a BVH bound of the template (or of a donor), with new
//! vertices, polygons, materials and hierarchy. Layout (measured on the
//! game's files, see also [`crate::ybn`]):
//!
//! * bound (0x150): `0x14` sphere radius, `0x20` box max, `0x30` box min,
//!   `0x40` box centre, `0x50` sphere centre; geometry: `0x84` vertex count,
//!   `0x88` polygons, `0x90` quantum, `0xA0` centre, `0xB0` vertices (3 × i16),
//!   `0xD0`/`0xD4` vertex and polygon counts, `0xF0` materials (8 bytes, the
//!   type first), `0x118` material index per polygon, `0x120` material count,
//!   `0x130` the hierarchy;
//! * polygon (16 bytes): its area (f32; the low 3 bits of the first byte are
//!   the polygon type, 0 = triangle), three vertex indices (u16), and per edge
//!   (corners k, k+1) the polygon on its other side (i16, −1: none);
//! * hierarchy (0x80): `0x00` nodes, count and capacity (u32); `0x20` box min,
//!   `0x30` max, `0x40` centre, `0x50` 1 / quantum, `0x60` quantum (box size
//!   / 65534); `0x70` subtree headers, count and capacity (u16);
//! * node (16 bytes): quantized box (6 × i16, around the centre), then for a
//!   leaf its first polygon and polygon count (at most 4; leaves cover the
//!   polygons in order), for an inner node the size of its subtree (its left
//!   child follows it, the right one follows the left subtree) and 0;
//! * subtree header (16 bytes): quantized box, first node and the node after
//!   the subtree: every subtree of at most 128 nodes whose parent is larger,
//!   in the order they are finished (as Bullet's quantized BVH does).
//!
//! A composite (type 10) keeps one child, the new bound: `0x70` children,
//! `0x78`/`0x80` transforms, `0x88` boxes, `0x90`/`0x98` flags, `0xA0`/`0xA2`
//! counts, `0xA8` its own hierarchy (none for one child).

use rage_resource::builder::{BlockId, Builder, Segment};
use rage_resource::{Resource, ResourceError};

use crate::ybn::kind;

const SYS: u64 = 0x5000_0000;
const GFX: u64 = 0x6000_0000;
const BVH_BOUND_SIZE: usize = 0x150;
const BVH_SIZE: usize = 0x80;
/// Most polygons in a leaf.
const LEAF_ITEMS: usize = 4;
/// Most nodes of a subtree with a header of its own (2048 bytes).
const SUBTREE_NODES: usize = 128;
/// Most vertices and polygons of one bound (indices are 15 bits).
pub const MAX_TRIANGLES: usize = 0x7FFF;

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

/// A collision mesh to write.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollisionMesh {
    pub positions: Vec<[f32; 3]>,
    /// Three per triangle.
    pub indices: Vec<u32>,
    /// The material type of each triangle (an index into the game's
    /// `materials.dat`; `None`: the template's first material), or empty:
    /// all the template's first.
    pub materials: Vec<Option<u8>>,
}

/// Where a resource keeps its collision: a `.ybn`'s root, a drawable's `0xC8`.
pub fn collision_of(res: &Resource, ydr: bool) -> Result<u64, ResourceError> {
    if !ydr {
        return Ok(SYS);
    }
    let at = res.u64(SYS + 0xC8)?;
    if !(SYS..GFX).contains(&at) {
        return Err(bad("the drawable has no collision"));
    }
    Ok(at)
}

/// The first BVH bound below `at` (itself or a composite's child).
fn find_bvh(res: &Resource, at: u64, depth: usize) -> Option<u64> {
    match res.u8(at + 0x10).ok()? {
        kind::BVH => Some(at),
        kind::COMPOSITE if depth < 4 => {
            let children = res.u64(at + 0x70).ok()?;
            (0..res.u16(at + 0xA0).ok()? as u64)
                .filter_map(|i| res.u64(children + i * 8).ok().filter(|&c| (SYS..GFX).contains(&c)))
                .find_map(|c| find_bvh(res, c, depth + 1))
        }
        _ => None,
    }
}

/// Whether a resource (`.ybn`, or `.ydr` with `ydr`) has a BVH bound to lend
/// its structure ([`replace_collision`]'s donor).
pub fn can_donate(res: &Resource, ydr: bool) -> bool {
    collision_of(res, ydr).ok().and_then(|at| find_bvh(res, at, 0)).is_some()
}

fn check_mesh(mesh: &CollisionMesh) -> Result<(), ResourceError> {
    let n = mesh.positions.len();
    if mesh.indices.is_empty() || !mesh.indices.len().is_multiple_of(3) {
        return Err(bad("the collision has no triangles"));
    }
    if mesh.indices.iter().any(|&i| i as usize >= n) {
        return Err(bad("the collision has triangles pointing at missing vertices"));
    }
    if !mesh.materials.is_empty() && mesh.materials.len() != mesh.indices.len() / 3 {
        return Err(bad("the collision has a material list of the wrong length"));
    }
    if mesh.positions.iter().flatten().any(|v| !v.is_finite()) {
        return Err(bad("the collision has positions that are not numbers"));
    }
    Ok(())
}

/// The resource and the BVH bound in it to copy the structure from: the template's, else the donor's.
fn structure_source<'a>(
    template: &'a Resource,
    root: u64,
    donor: Option<(&'a Resource, bool)>,
) -> Result<(&'a Resource, u64), ResourceError> {
    if let Some(at) = find_bvh(template, root, 0) {
        return Ok((template, at));
    }
    let (d, d_ydr) = donor
        .ok_or_else(|| bad("the original collision has no triangle mesh to copy the structure from (give a donor)"))?;
    let at = collision_of(d, d_ydr).ok().and_then(|r| find_bvh(d, r, 0));
    Ok((d, at.ok_or_else(|| bad("the donor has no triangle mesh collision"))?))
}

/// The `len` bytes the pointer at `at` leads to, or zeroes where it is null.
fn bytes_or_zero(res: &Resource, at: u64, len: usize) -> Result<Vec<u8>, ResourceError> {
    let p = res.u64(at)?;
    Ok(if p != 0 { res.bytes(p, len)?.to_vec() } else { vec![0; len] })
}

/// The new bound and the system block it goes into.
struct NewBound<'a> {
    sys: BlockId,
    bound: BlockId,
    built: &'a Built,
}

/// Makes the new bound the one child of the composite at `root`, without moving it.
fn attach_to_composite(b: &mut Builder, template: &Resource, root: u64, new: &NewBound) -> Result<(), ResourceError> {
    let (sys, built) = (new.sys, new.built);
    let at = (root - SYS) as usize;
    let flags = bytes_or_zero(template, root + 0x90, 8)?;
    let mut transform = bytes_or_zero(template, root + 0x78, 64)?;
    for (k, v) in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0].iter().enumerate() {
        // Columns of three; the w of each column holds other data.
        let at = (k / 3) * 16 + (k % 3) * 4;
        transform[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut aabb = bytes_or_zero(template, root + 0x88, 32)?;
    put_vec3(&mut aabb, 0, built.min);
    put_vec3(&mut aabb, 16, built.max);
    let children = b.add(Segment::System, vec![0u8; 16]);
    b.pointer(children, 0, new.bound, 0);
    let transform = b.add(Segment::System, transform);
    let aabb = b.add(Segment::System, aabb);
    let flags = b.add(Segment::System, [flags, vec![0; 8]].concat());
    b.pointer(sys, at + 0x70, children, 0);
    b.pointer(sys, at + 0x78, transform, 0);
    b.pointer(sys, at + 0x80, transform, 0);
    b.pointer(sys, at + 0x88, aabb, 0);
    b.pointer(sys, at + 0x90, flags, 0);
    b.pointer(sys, at + 0x98, flags, 0);
    let d = b.data_mut(sys);
    put_u16(d, at + 0xA0, 1);
    put_u16(d, at + 0xA2, 1);
    d[at + 0xA8..at + 0xB0].fill(0);
    put_bound_box(d, at, built);
    Ok(())
}

/// The structures copied from a BVH bound: the bound, its hierarchy and its first material.
struct Structure {
    bound: Vec<u8>,
    bvh: Vec<u8>,
    material: Vec<u8>,
}

fn copy_structure(src: &Resource, src_at: u64) -> Result<Structure, ResourceError> {
    let bound = src.bytes(src_at, BVH_BOUND_SIZE)?.to_vec();
    let bvh_at = src.u64(src_at + 0x130)?;
    let bvh = src.bytes(bvh_at, BVH_SIZE)?.to_vec();
    let material = bytes_or_zero(src, src_at + 0xF0, 8)?;
    Ok(Structure { bound, bvh, material })
}

/// The template's file with the bound built from `built` as its collision.
fn assemble(
    template: &Resource,
    ydr: bool,
    root: u64,
    built: &Built,
    structure: Structure,
) -> Result<Vec<u8>, ResourceError> {
    let mut b = Builder::new();
    let sys = b.add(Segment::System, template.system.clone());
    if !template.graphics.is_empty() {
        b.add(Segment::Graphics, template.graphics.clone());
    }
    b.add_pages_info(sys, 0x08);
    let bound = write_bvh_bound(&mut b, built, structure.bound, structure.bvh, &structure.material);

    if template.u8(root + 0x10)? == kind::COMPOSITE {
        attach_to_composite(&mut b, template, root, &NewBound { sys, bound, built })?;
    } else if ydr {
        b.pointer(sys, 0xC8, bound, 0);
    } else {
        return Err(bad("the collision file's root is no composite: it cannot get a new bound"));
    }
    b.build(template.version)
}

/// The template's file with its collision replaced by `mesh` (`ydr`: the
/// template is a drawable). The structures are copied from a BVH bound of the
/// template, else of `donor` (a `.ybn`, or a `.ydr` with `donor_ydr`).
pub fn replace_collision(
    template: &Resource,
    ydr: bool,
    mesh: &CollisionMesh,
    donor: Option<(&Resource, bool)>,
) -> Result<Vec<u8>, ResourceError> {
    let root = collision_of(template, ydr)?;
    check_mesh(mesh)?;
    let (src, src_at) = structure_source(template, root, donor)?;
    let structure = copy_structure(src, src_at)?;
    let built = build_geometry(mesh, structure.material[0])?;
    assemble(template, ydr, root, &built, structure)
}

/// Vertices, polygons and hierarchy of a mesh, in the bound's order.
struct Built {
    min: [f32; 3],
    max: [f32; 3],
    quantum: [f32; 3],
    centre: [f32; 3],
    vertices: Vec<[i16; 3]>,
    /// (corners, area, neighbours, material index)
    polygons: Vec<([u16; 3], f32, [i16; 3], u8)>,
    materials: Vec<u8>,
    nodes: Vec<Node>,
    subtrees: Vec<(usize, usize)>,
    bvh_quantum: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    /// Leaf: first polygon and count; inner: subtree size and 0.
    item: usize,
    count: usize,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn build_geometry(mesh: &CollisionMesh, default_material: u8) -> Result<Built, ResourceError> {
    let tris: Vec<[usize; 3]> =
        mesh.indices.as_chunks::<3>().0.iter().map(|t| [t[0] as usize, t[1] as usize, t[2] as usize]).collect();
    if tris.len() > MAX_TRIANGLES {
        return Err(bad(format!("the collision has {} triangles; at most {MAX_TRIANGLES}", tris.len())));
    }
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for t in &tris {
        for &i in t {
            for k in 0..3 {
                min[k] = min[k].min(mesh.positions[i][k]);
                max[k] = max[k].max(mesh.positions[i][k]);
            }
        }
    }
    let centre: [f32; 3] = std::array::from_fn(|k| (min[k] + max[k]) / 2.0);
    let quantum: [f32; 3] = std::array::from_fn(|k| ((max[k] - min[k]) / 65534.0).max(1e-6));
    // Vertices once per quantized place, so neighbours share them.
    let mut vertices: Vec<[i16; 3]> = Vec::new();
    let mut seen: std::collections::HashMap<[i16; 3], u16> = std::collections::HashMap::new();
    let mut corner = |p: [f32; 3]| -> Result<u16, ResourceError> {
        let q: [i16; 3] =
            std::array::from_fn(|k| ((p[k] - centre[k]) / quantum[k]).round().clamp(-32767.0, 32767.0) as i16);
        if let Some(&i) = seen.get(&q) {
            return Ok(i);
        }
        if vertices.len() >= MAX_TRIANGLES {
            return Err(bad(format!("the collision has more than {MAX_TRIANGLES} vertices")));
        }
        vertices.push(q);
        seen.insert(q, vertices.len() as u16 - 1);
        Ok(vertices.len() as u16 - 1)
    };
    let mut polys: Vec<[u16; 3]> = Vec::with_capacity(tris.len());
    for t in &tris {
        polys.push([corner(mesh.positions[t[0]])?, corner(mesh.positions[t[1]])?, corner(mesh.positions[t[2]])?]);
    }
    let dequant = |v: [i16; 3]| -> [f32; 3] { std::array::from_fn(|k| v[k] as f32 * quantum[k] + centre[k]) };
    // Material types in use, each once.
    let mut materials: Vec<u8> = Vec::new();
    let poly_material: Vec<u8> = (0..tris.len())
        .map(|i| {
            let m = mesh.materials.get(i).copied().flatten().unwrap_or(default_material);
            match materials.iter().position(|&x| x == m) {
                Some(k) => k as u8,
                None => {
                    materials.push(m);
                    materials.len() as u8 - 1
                }
            }
        })
        .collect();
    if materials.len() > 255 {
        return Err(bad("the collision uses more than 255 materials"));
    }
    // The hierarchy over polygon boxes; it gives the polygons' order.
    let boxes: Vec<([f32; 3], [f32; 3])> = polys
        .iter()
        .map(|p| {
            let c = p.map(|i| dequant(vertices[i as usize]));
            (
                std::array::from_fn(|k| c[0][k].min(c[1][k]).min(c[2][k])),
                std::array::from_fn(|k| c[0][k].max(c[1][k]).max(c[2][k])),
            )
        })
        .collect();
    let mut order: Vec<usize> = (0..polys.len()).collect();
    let mut nodes = Vec::new();
    let mut subtrees = Vec::new();
    build_nodes(&boxes, &mut order, 0, &mut nodes, &mut subtrees);
    if nodes.len() <= SUBTREE_NODES {
        subtrees = vec![(0, nodes.len())];
    }
    if nodes.len() > 0x7FFF {
        return Err(bad("the collision's hierarchy is too large"));
    }
    // Polygons in leaf order, with their neighbours.
    let ordered: Vec<[u16; 3]> = order.iter().map(|&i| polys[i]).collect();
    let mut edges: std::collections::HashMap<(u16, u16), Vec<usize>> = std::collections::HashMap::new();
    for (i, p) in ordered.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let polygons = ordered
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let c = p.map(|v| dequant(vertices[v as usize]));
            let n = cross(sub(c[1], c[0]), sub(c[2], c[0]));
            let area = 0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let neighbours: [i16; 3] = std::array::from_fn(|k| {
                let (a, b) = (p[k], p[(k + 1) % 3]);
                edges[&(a.min(b), a.max(b))].iter().find(|&&j| j != i).map_or(-1, |&j| j as i16)
            });
            (*p, area, neighbours, poly_material[order[i]])
        })
        .collect();
    let bvh_quantum: [f32; 3] = std::array::from_fn(|k| ((max[k] - min[k]) / 65534.0).max(1e-6));
    Ok(Built { min, max, quantum, centre, vertices, polygons, materials, nodes, subtrees, bvh_quantum })
}

/// Builds the nodes of `order` (polygon indices, reordered in place to leaf
/// order; `first` is where they start among all polygons), depth first.
/// Returns the index of the subtree's root.
fn build_nodes(
    boxes: &[([f32; 3], [f32; 3])],
    order: &mut [usize],
    first: usize,
    nodes: &mut Vec<Node>,
    subtrees: &mut Vec<(usize, usize)>,
) -> usize {
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for &i in order.iter() {
        for k in 0..3 {
            min[k] = min[k].min(boxes[i].0[k]);
            max[k] = max[k].max(boxes[i].1[k]);
        }
    }
    let at = nodes.len();
    if order.len() <= LEAF_ITEMS {
        nodes.push(Node { min, max, item: first, count: order.len() });
        return at;
    }
    nodes.push(Node { min, max, item: 0, count: 0 });
    // Split at the middle along the longest axis of the centres.
    let centre = |i: usize, k: usize| (boxes[i].0[k] + boxes[i].1[k]) / 2.0;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for &i in order.iter() {
        for k in 0..3 {
            lo[k] = lo[k].min(centre(i, k));
            hi[k] = hi[k].max(centre(i, k));
        }
    }
    let axis = (0..3).max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b]))).unwrap_or(0);
    order.sort_by(|&a, &b| centre(a, axis).total_cmp(&centre(b, axis)));
    let half = order.len() / 2;
    let (left, right) = order.split_at_mut(half);
    let l = build_nodes(boxes, left, first, nodes, subtrees);
    let r = build_nodes(boxes, right, first + half, nodes, subtrees);
    let size = nodes.len() - at;
    nodes[at].item = size;
    // Subtrees small enough under a node too large get headers.
    if size > SUBTREE_NODES {
        for child in [l, r] {
            let child_size = if nodes[child].count > 0 { 1 } else { nodes[child].item };
            if child_size <= SUBTREE_NODES {
                subtrees.push((child, child + child_size));
            }
        }
    }
    at
}

fn put_u16(d: &mut [u8], at: usize, v: u16) {
    d[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(d: &mut [u8], at: usize, v: f32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_vec3(d: &mut [u8], at: usize, v: [f32; 3]) {
    for (k, x) in v.iter().enumerate() {
        put_f32(d, at + k * 4, *x);
    }
}

/// A bound's box, box centre, sphere centre and radius.
fn put_bound_box(d: &mut [u8], at: usize, b: &Built) {
    let centre: [f32; 3] = std::array::from_fn(|k| (b.min[k] + b.max[k]) / 2.0);
    let half = sub(b.max, centre);
    put_vec3(d, at + 0x20, b.max);
    put_vec3(d, at + 0x30, b.min);
    put_vec3(d, at + 0x40, centre);
    put_vec3(d, at + 0x50, centre);
    put_f32(d, at + 0x14, (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt());
}

fn write_bvh_bound(b: &mut Builder, g: &Built, mut bound: Vec<u8>, mut bvh: Vec<u8>, material: &[u8]) -> BlockId {
    let verts: Vec<u8> = g.vertices.iter().flat_map(|v| v.iter().flat_map(|x| x.to_le_bytes())).collect();
    let mut polys = Vec::with_capacity(g.polygons.len() * 16);
    for (corners, area, neighbours, _) in &g.polygons {
        let mut p = [0u8; 16];
        put_f32(&mut p, 0, *area);
        p[0] &= !7; // triangle
        for k in 0..3 {
            put_u16(&mut p, 4 + k * 2, corners[k]);
            p[10 + k * 2..12 + k * 2].copy_from_slice(&neighbours[k].to_le_bytes());
        }
        polys.extend_from_slice(&p);
    }
    let poly_materials: Vec<u8> = g.polygons.iter().map(|p| p.3).collect();
    let materials: Vec<u8> = g
        .materials
        .iter()
        .flat_map(|&m| {
            let mut x = material.to_vec();
            x[0] = m;
            x
        })
        .collect();

    // Hierarchy.
    let bq = g.bvh_quantum;
    let bc: [f32; 3] = std::array::from_fn(|k| (g.min[k] + g.max[k]) / 2.0);
    let quant = |v: [f32; 3], up: bool| -> [i16; 3] {
        std::array::from_fn(|k| {
            let x = (v[k] - bc[k]) / bq[k];
            (if up { x.ceil() } else { x.floor() }).clamp(-32767.0, 32767.0) as i16
        })
    };
    let mut nodes = Vec::with_capacity(g.nodes.len() * 16);
    for n in &g.nodes {
        for v in quant(n.min, false).into_iter().chain(quant(n.max, true)) {
            nodes.extend_from_slice(&v.to_le_bytes());
        }
        nodes.extend_from_slice(&(n.item as u16).to_le_bytes());
        nodes.extend_from_slice(&(n.count as u16).to_le_bytes());
    }
    let mut trees = Vec::with_capacity(g.subtrees.len() * 16);
    for &(start, end) in &g.subtrees {
        let n = g.nodes[start];
        for v in quant(n.min, false).into_iter().chain(quant(n.max, true)) {
            trees.extend_from_slice(&v.to_le_bytes());
        }
        trees.extend_from_slice(&(start as u16).to_le_bytes());
        trees.extend_from_slice(&(end as u16).to_le_bytes());
    }
    put_vec3(&mut bvh, 0x20, g.min);
    put_vec3(&mut bvh, 0x30, g.max);
    put_vec3(&mut bvh, 0x40, bc);
    put_vec3(&mut bvh, 0x50, std::array::from_fn(|k| 1.0 / bq[k]));
    put_vec3(&mut bvh, 0x60, bq);
    put_u32(&mut bvh, 0x08, g.nodes.len() as u32);
    put_u32(&mut bvh, 0x0C, g.nodes.len() as u32);
    put_u16(&mut bvh, 0x78, g.subtrees.len() as u16);
    put_u16(&mut bvh, 0x7A, g.subtrees.len() as u16);

    put_bound_box(&mut bound, 0, g);
    bound[0x70..0x80].fill(0); // no shrunk vertices
    put_u32(&mut bound, 0x84, g.vertices.len() as u32);
    put_vec3(&mut bound, 0x90, g.quantum);
    put_vec3(&mut bound, 0xA0, g.centre);
    bound[0xB8..0xD0].fill(0); // no vertex colours, octants
    put_u32(&mut bound, 0xD0, g.vertices.len() as u32);
    put_u32(&mut bound, 0xD4, g.polygons.len() as u32);
    bound[0xF8..0x100].fill(0); // no material colours
    bound[0x120] = g.materials.len() as u8;
    bound[0x121] = 0;

    let pad = |mut v: Vec<u8>| {
        v.resize(v.len().div_ceil(16).max(1) * 16, 0);
        v
    };
    let verts = b.add(Segment::System, pad(verts));
    let polys = b.add(Segment::System, pad(polys));
    let materials = b.add(Segment::System, pad(materials));
    let poly_materials = b.add(Segment::System, pad(poly_materials));
    let nodes = b.add(Segment::System, pad(nodes));
    let trees = b.add(Segment::System, pad(trees));
    let bvh = b.add(Segment::System, bvh);
    b.pointer(bvh, 0x00, nodes, 0);
    b.pointer(bvh, 0x70, trees, 0);
    let bound = b.add(Segment::System, bound);
    b.pointer(bound, 0x88, polys, 0);
    b.pointer(bound, 0xB0, verts, 0);
    b.pointer(bound, 0xF0, materials, 0);
    b.pointer(bound, 0x118, poly_materials, 0);
    b.pointer(bound, 0x130, bvh, 0);
    bound
}

/// The hierarchy of a BVH bound, as read for [`check_bvh`].
struct BvhView<'a> {
    res: &'a Resource,
    /// The bound.
    at: u64,
    /// The hierarchy.
    bvh: u64,
    /// The nodes, 16 bytes each.
    raw: &'a [u8],
    count: usize,
    centre: [f32; 3],
    quantum: [f32; 3],
}

/// A node: its quantized box, then its item (first polygon or subtree size) and count.
type RawNode = ([i16; 6], usize, usize);

/// Where the vertices and polygons of a bound are, and how its vertices are quantized.
struct LeafTables {
    verts_at: u64,
    polys_at: u64,
    vq: [f32; 3],
    vc: [f32; 3],
}

impl<'a> BvhView<'a> {
    /// The hierarchy of the bound at `at`, and the bound's polygon count.
    fn open(res: &'a Resource, at: u64) -> Result<(Self, usize), ResourceError> {
        let bvh = res.u64(at + 0x130)?;
        let polys = res.u32(at + 0xD4)? as usize;
        let nodes_at = res.u64(bvh)?;
        let count = res.u32(bvh + 8)? as usize;
        let f = |o: u64| -> [f32; 3] { [0, 4, 8].map(|k| f32::from_bits(res.u32(bvh + o + k).unwrap_or(0))) };
        let (centre, quantum) = (f(0x40), f(0x60));
        let raw = res.bytes(nodes_at, count * 16)?;
        Ok((Self { res, at, bvh, raw, count, centre, quantum }, polys))
    }

    fn node(&self, i: usize) -> RawNode {
        let b = &self.raw[i * 16..i * 16 + 16];
        let s = |k: usize| i16::from_le_bytes([b[k], b[k + 1]]);
        ([s(0), s(2), s(4), s(6), s(8), s(10)], s(12) as u16 as usize, s(14) as u16 as usize)
    }

    /// Sizes and leaf order, depth first; the leaves must cover all `polys` polygons.
    fn check_structure(&self, polys: usize) -> Result<(), ResourceError> {
        let mut next = 0usize;
        let mut stack = vec![(0usize, self.count)];
        while let Some((i, end)) = stack.pop() {
            let Some(left) = self.visit(i, end, &mut next)? else { continue };
            // Right first on the stack, so the left is checked first.
            stack.push((i + 1 + left, end));
            stack.push((i + 1, i + 1 + left));
        }
        if next != polys {
            return Err(bad(format!("leaves cover {next} of {polys} polygons")));
        }
        Ok(())
    }

    /// Checks node `i` of a subtree ending at `end`: a leaf takes its polygons from `next` (and gives `None`),
    /// an inner node gives the size of its left subtree.
    fn visit(&self, i: usize, end: usize, next: &mut usize) -> Result<Option<usize>, ResourceError> {
        if i >= end || i >= self.count {
            return Err(bad(format!("node {i} lies outside its parent")));
        }
        let (_, item, n) = self.node(i);
        if n > 0 {
            if n > LEAF_ITEMS || item != *next || end != i + 1 {
                return Err(bad(format!("leaf {i}: polygons {item}+{n}, expected {next}, in a subtree to {end}")));
            }
            *next += n;
            return Ok(None);
        }
        if i + item != end || item < 3 {
            return Err(bad(format!("node {i}: subtree of {item} nodes, its place ends at {end}")));
        }
        let (_, litem, ln) = self.node(i + 1);
        Ok(Some(if ln > 0 { 1 } else { litem }))
    }

    /// Leaf boxes around their polygons' vertices.
    fn check_leaf_boxes(&self) -> Result<(), ResourceError> {
        let (res, at) = (self.res, self.at);
        let verts_at = res.u64(at + 0xB0)?;
        let vq = [0, 4, 8].map(|k| f32::from_bits(res.u32(at + 0x90 + k).unwrap_or(0)));
        let vc = [0, 4, 8].map(|k| f32::from_bits(res.u32(at + 0xA0 + k).unwrap_or(0)));
        let polys_at = res.u64(at + 0x88)?;
        let tables = LeafTables { verts_at, polys_at, vq, vc };
        (0..self.count).try_for_each(|i| self.check_leaf(&tables, i))
    }

    fn check_leaf(&self, t: &LeafTables, i: usize) -> Result<(), ResourceError> {
        let (q, item, n) = self.node(i);
        if n == 0 {
            return Ok(());
        }
        let lo: [f32; 3] = std::array::from_fn(|k| q[k] as f32 * self.quantum[k] + self.centre[k]);
        let hi: [f32; 3] = std::array::from_fn(|k| q[k + 3] as f32 * self.quantum[k] + self.centre[k]);
        for p in item..item + n {
            if self.polygon_outside(t, p, (lo, hi))? {
                return Err(bad(format!("polygon {p} lies outside leaf {i}")));
            }
        }
        Ok(())
    }

    /// Whether a vertex of triangle `p` lies outside the leaf box `(lo, hi)`.
    fn polygon_outside(&self, t: &LeafTables, p: usize, (lo, hi): ([f32; 3], [f32; 3])) -> Result<bool, ResourceError> {
        let b = self.res.bytes(t.polys_at + p as u64 * 16, 16)?;
        if b[0] & 7 != 0 {
            return Ok(false); // boxes and spheres reach out by a radius
        }
        for o in [4, 6, 8] {
            let v = u16::from_le_bytes([b[o], b[o + 1]]) & 0x7FFF;
            let c = self.res.bytes(t.verts_at + v as u64 * 6, 6)?;
            let outside = (0..3).any(|k| {
                let x = i16::from_le_bytes([c[k * 2], c[k * 2 + 1]]) as f32 * t.vq[k] + t.vc[k];
                let slack = self.quantum[k] * 2.0 + 1e-3;
                x < lo[k] - slack || x > hi[k] + slack
            });
            if outside {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Collects the headers of the subtrees below node `i` that get one, gives the size of its subtree.
    fn subtree_headers(&self, i: usize, out: &mut Vec<(usize, usize)>) -> usize {
        let (_, item, n) = self.node(i);
        if n > 0 {
            return 1;
        }
        let l = self.subtree_headers(i + 1, out);
        let r = self.subtree_headers(i + 1 + l, out);
        if item > SUBTREE_NODES {
            for (c, size) in [(i + 1, l), (i + 1 + l, r)] {
                if size <= SUBTREE_NODES {
                    out.push((c, c + size));
                }
            }
        }
        item
    }

    /// Subtree headers: Bullet's rule.
    fn check_subtree_headers(&self) -> Result<(), ResourceError> {
        let res = self.res;
        let mut want = Vec::new();
        let total = self.subtree_headers(0, &mut want);
        if total <= SUBTREE_NODES {
            want = vec![(0, total)];
        }
        let trees_at = res.u64(self.bvh + 0x70)?;
        let trees = res.u16(self.bvh + 0x78)? as usize;
        let got: Vec<(usize, usize)> = (0..trees)
            .map(|t| {
                let b = res.bytes(trees_at + t as u64 * 16, 16)?;
                Ok((u16::from_le_bytes([b[12], b[13]]) as usize, u16::from_le_bytes([b[14], b[15]]) as usize))
            })
            .collect::<Result<_, ResourceError>>()?;
        if got != want {
            return Err(bad(format!("{} subtree headers, expected {}", got.len(), want.len())));
        }
        Ok(())
    }
}

/// Checks the hierarchy of the BVH bound at `at` as the game builds them
/// (see the module): leaves of at most four polygons covering all of them in
/// order, subtree sizes that add up, boxes around their polygons, and the
/// subtree headers. Run on the game's own files too, to check this reading.
pub fn check_bvh(res: &Resource, at: u64) -> Result<(), ResourceError> {
    let (view, polys) = BvhView::open(res, at)?;
    view.check_structure(polys)?;
    view.check_leaf_boxes()?;
    view.check_subtree_headers()
}

/// Every BVH bound below `at`, for [`check_bvh`].
pub fn bvh_bounds(res: &Resource, at: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut stack = vec![(at, 0)];
    while let Some((b, depth)) = stack.pop() {
        match res.u8(b + 0x10) {
            Ok(kind::BVH) => out.push(b),
            Ok(kind::COMPOSITE) if depth < 4 => {
                let (Ok(children), Ok(n)) = (res.u64(b + 0x70), res.u16(b + 0xA0)) else { continue };
                for i in 0..n as u64 {
                    if let Ok(c) = res.u64(children + i * 8)
                        && (SYS..GFX).contains(&c)
                    {
                        stack.push((c, depth + 1));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_hierarchies_the_game_way() {
        // A grid of 40 × 40 squares (3200 triangles).
        let n = 40;
        let mut mesh = CollisionMesh::default();
        for y in 0..=n {
            for x in 0..=n {
                mesh.positions.push([x as f32, y as f32, ((x * y) % 7) as f32 * 0.1]);
            }
        }
        for y in 0..n {
            for x in 0..n {
                let i = (y * (n + 1) + x) as u32;
                let w = n as u32 + 1;
                mesh.indices.extend([i, i + 1, i + w + 1, i, i + w + 1, i + w]);
                mesh.materials.extend([Some((x % 3) as u8), Some(9)]);
            }
        }
        let g = build_geometry(&mesh, 0).unwrap();
        assert_eq!(g.polygons.len(), 3200);
        assert_eq!(g.vertices.len(), 41 * 41, "shared corners once");
        assert_eq!(g.materials.len(), 4);
        // Inside triangles have three neighbours, sharing the edge.
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
        // Leaves cover the polygons in order; sizes add up.
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
}
