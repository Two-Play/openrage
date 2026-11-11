//! Paths as display meshes: path nodes (`.ynd`), vehicle recordings (`.yvr`)
//! and waypoint recordings (`.ywr`).
//!
//! * `.ynd` (`CPathRegionData`): nodes `0x10` (count u32 `0x18`, vehicle nodes
//!   first, their count `0x1C`), links `0x28` (count `0x30`). A node (40 bytes)
//!   has area `0x10`, id `0x12`, first link `0x1A`, position x `0x1C`, y `0x1E`
//!   (i16 / 4), z `0x22` (i16 / 32), link count in the top 5 bits of `0x25`.
//!   A link (8 bytes) points to (area, node id).
//! * `.yvr`: entries `0x10` (u64 pointer, u16 count), 32 bytes each with the
//!   time at 0 and the position at `0x14`.
//! * `.ywr`: entries `0x18`, count u32 `0x20`, 20 bytes each, position first.

use rage_resource::{Resource, ResourceError};

use crate::drawable::{Drawable, Mesh, Shader};
use crate::geometry::Builder;
use crate::shaders;

/// Material hashes of the parts of a path: this + the part.
pub const MATERIAL_BASE: u32 = 0xC0A7_0000;

mod part {
    pub const VEHICLE_NODES: u8 = 0;
    pub const PED_NODES: u8 = 1;
    pub const VEHICLE_LINKS: u8 = 2;
    pub const PED_LINKS: u8 = 3;
    pub const ROUTE: u8 = 4;
    pub const POINTS: u8 = 5;
    pub const ENDS: u8 = 6;
}

const PARTS: [(&str, [f32; 4]); 7] = [
    ("Vehicle nodes", [0.25, 0.55, 0.95, 1.0]),
    ("Pedestrian nodes", [0.95, 0.6, 0.2, 1.0]),
    ("Vehicle links", [0.55, 0.75, 1.0, 1.0]),
    ("Pedestrian links", [1.0, 0.8, 0.5, 1.0]),
    ("Route", [0.3, 0.8, 0.45, 1.0]),
    ("Recorded points", [0.85, 0.85, 0.3, 1.0]),
    ("Start and end", [0.9, 0.3, 0.35, 1.0]),
];

/// What a path file contains.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub nodes: usize,
    pub vehicle_nodes: usize,
    pub links: usize,
    /// Recorded points (`.yvr`, `.ywr`).
    pub points: usize,
    /// Length of a recording in seconds (`.yvr`).
    pub duration: f32,
    /// Recorded points left out because they lie far outside the map (a few
    /// of the game's recordings contain garbage entries).
    pub skipped: usize,
}

/// Whether a recorded position can be on the map.
fn plausible(p: [f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() < 20000.0)
}

fn finish(b: Builder, name: &str) -> Drawable {
    for (i, (label, color)) in PARTS.iter().enumerate() {
        shaders::register(MATERIAL_BASE + i as u32, *label, *color);
    }
    let mut d = Drawable { name: name.into(), ..Default::default() };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut meshes = b.meshes;
    meshes.sort_by_key(|(m, _)| *m);
    for (i, (material, mesh)) in meshes.into_iter().enumerate() {
        for p in &mesh.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        d.shaders.push(Shader { name_hash: MATERIAL_BASE + material as u32, ..Default::default() });
        d.meshes.push(Mesh { shader: i as u16, ..mesh });
    }
    if lo[0] <= hi[0] {
        (d.bounds_min, d.bounds_max) = (lo, hi);
    }
    d
}

/// Size of markers and bars: thin enough for a street, thick enough to see
/// when a whole region is in view.
fn line_size(points: impl Iterator<Item = [f32; 3]>) -> f32 {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in points {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if extent.is_finite() { (extent / 300.0).clamp(0.4, 8.0) } else { 0.4 }
}

fn vec3(res: &Resource, at: u64) -> Result<[f32; 3], ResourceError> {
    let b = res.bytes(at, 12)?;
    Ok([0, 4, 8].map(|o| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])))
}

/// Path nodes and their links (`.ynd`).
pub fn parse_ynd(res: &Resource) -> Result<(Drawable, Stats), ResourceError> {
    let root = Resource::ROOT;
    let nodes_at = res.u64(root + 0x10)?;
    let count = res.u32(root + 0x18)? as u64;
    let vehicle = res.u32(root + 0x1C)? as u64;
    let links_at = res.u64(root + 0x28)?;
    let link_count = res.u32(root + 0x30)? as u64;
    if count > 1 << 16 || link_count > 1 << 18 {
        return Err(ResourceError::Format(format!("implausible path data: {count} nodes, {link_count} links")));
    }
    let i16_at = |at: u64| -> Result<f32, ResourceError> { Ok(res.u16(at)? as i16 as f32) };
    let mut nodes = Vec::with_capacity(count as usize);
    for i in 0..count {
        let n = nodes_at + i * 40;
        let pos = [i16_at(n + 0x1C)? / 4.0, i16_at(n + 0x1E)? / 4.0, i16_at(n + 0x22)? / 32.0];
        let area = res.u16(n + 0x10)?;
        let first = res.u16(n + 0x1A)? as u64;
        let links = (res.u8(n + 0x25)? >> 3) as u64;
        nodes.push((pos, area, first, links));
    }
    let mut b = Builder::default();
    let mut stats = Stats { nodes: nodes.len(), vehicle_nodes: vehicle as usize, ..Default::default() };
    let size = line_size(nodes.iter().map(|n| n.0));
    for (i, &(pos, area, first, links)) in nodes.iter().enumerate() {
        let is_vehicle = (i as u64) < vehicle;
        b.marker(if is_vehicle { part::VEHICLE_NODES } else { part::PED_NODES }, pos, size * 1.5);
        for l in first..(first + links).min(link_count) {
            let at = links_at + l * 8;
            let (to_area, to_node) = (res.u16(at)?, res.u16(at + 2)? as usize);
            stats.links += 1;
            // Links into other areas need that area's file; each link within
            // the area is drawn once.
            if to_area == area
                && to_node > i
                && let Some(&(to, ..)) = nodes.get(to_node)
            {
                b.bar(if is_vehicle { part::VEHICLE_LINKS } else { part::PED_LINKS }, pos, to, size * 0.6);
            }
        }
    }
    Ok((finish(b, "path nodes"), stats))
}

fn route(points: &[[f32; 3]], name: &str) -> Drawable {
    let mut b = Builder::default();
    let size = line_size(points.iter().copied());
    for w in points.windows(2) {
        b.bar(part::ROUTE, w[0], w[1], size);
    }
    // A marker every few points, larger ones at start and end.
    let step = (points.len() / 64).max(1);
    for (i, p) in points.iter().enumerate() {
        if i == 0 || i + 1 == points.len() {
            b.marker(part::ENDS, *p, size * 3.0);
        } else if i % step == 0 {
            b.marker(part::POINTS, *p, size * 1.2);
        }
    }
    finish(b, name)
}

/// A vehicle recording (`.yvr`): the route driven.
pub fn parse_yvr(res: &Resource) -> Result<(Drawable, Stats), ResourceError> {
    let root = Resource::ROOT;
    let at = res.u64(root + 0x10)?;
    let count = res.u16(root + 0x18)? as u64;
    let mut points = Vec::with_capacity(count as usize);
    let (mut last_time, mut skipped) = (0, 0);
    for i in 0..count {
        let e = at + i * 32;
        let p = vec3(res, e + 0x14)?;
        if plausible(p) {
            last_time = res.u32(e)?;
            points.push(p);
        } else {
            skipped += 1;
        }
    }
    let stats = Stats { points: points.len(), duration: last_time as f32 / 1000.0, skipped, ..Default::default() };
    Ok((route(&points, "vehicle recording"), stats))
}

/// A waypoint recording (`.ywr`): the route walked.
pub fn parse_ywr(res: &Resource) -> Result<(Drawable, Stats), ResourceError> {
    let root = Resource::ROOT;
    let at = res.u64(root + 0x18)?;
    let count = res.u32(root + 0x20)? as u64;
    if count > 1 << 16 {
        return Err(ResourceError::Format(format!("implausible waypoint count {count}")));
    }
    let all: Vec<[f32; 3]> = (0..count).map(|i| vec3(res, at + i * 20)).collect::<Result<_, _>>()?;
    let points: Vec<[f32; 3]> = all.iter().copied().filter(|p| plausible(*p)).collect();
    let stats = Stats { points: points.len(), skipped: all.len() - points.len(), ..Default::default() };
    Ok((route(&points, "waypoint recording"), stats))
}

/// Reads a path file by its extension (`ynd`, `yvr`, `ywr`).
pub fn parse(res: &Resource, extension: &str) -> Result<(Drawable, Stats), ResourceError> {
    match extension.to_ascii_lowercase().as_str() {
        "ynd" => parse_ynd(res),
        "yvr" => parse_yvr(res),
        "ywr" => parse_ywr(res),
        other => Err(ResourceError::Format(format!("not a path file: .{other}"))),
    }
}

pub fn is_path_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".ynd") || lower.ends_with(".yvr") || lower.ends_with(".ywr")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_has_a_bar_per_step_and_end_markers() {
        let d = route(&[[0.0; 3], [5.0, 0.0, 0.0], [5.0, 5.0, 0.0]], "test");
        let labels: Vec<String> = d.shaders.iter().map(|s| shaders::label(s.name_hash)).collect();
        assert_eq!(labels, vec!["Route", "Recorded points", "Start and end"]);
        assert_eq!(d.meshes[0].indices.len(), 2 * 36);
        assert_eq!(d.bounds_min[0], -1.2);
        assert_eq!(line_size([[0.0; 3], [3000.0, 0.0, 0.0]].into_iter()), 8.0);
    }
}
