//! GTA VI collisions (`.#bn`), turned into meshes for display.
//!
//! Like GTA V's ([`crate::ybn`]), with other numbers for the kinds and places
//! of some fields. A bound has its kind at 16. Two kinds were seen:
//! - 6, a composite: child pointers at `0x70`, transforms (4×4 floats) at
//!   `0x78`, child count (16 bits) at `0x98`.
//! - 5, geometry: polygons at `0x88` (16 bytes each), quantum at `0x90`,
//!   centre at `0xA0`, vertices at `0xB0` (3 × i16, × quantum + centre),
//!   vertex and polygon counts at `0xD0` and `0xD4`, materials at `0xF0`
//!   (8 bytes, type in the low 16 bits), material number per polygon at
//!   `0x118`, material count at `0x120`.
//!
//! A polygon has its kind in byte 3. 0 is a triangle: vertex numbers at 4, 6
//! and 8. 2 and 4 are a capsule and a cylinder: vertices at 0 and 8, radius
//! at 4. Others were not seen and are counted as unknown.

use rage_core::bytes::LeRead;

use crate::drawable::{Drawable, Mesh, Shader};
use crate::geometry::Builder;
use crate::math::{self, Mat4};
use crate::ybn::MATERIAL_BASE;

const SYSTEM: u64 = 0x5000_0000;
const MAX_BOUNDS: usize = 100_000;
const MAX_POLYGONS: usize = 10_000_000;

#[derive(Debug, thiserror::Error)]
#[error("damaged or unknown collision data: {0}")]
pub struct BnError(&'static str);

/// What a collision file contains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub bounds: usize,
    pub triangles: usize,
    /// Capsules and cylinders.
    pub primitives: usize,
    /// Bounds and polygons of an unknown kind, which are left out.
    pub unknown: usize,
    /// Material types used (16-bit numbers; their names are not known).
    pub materials: Vec<u16>,
}

struct Reader<'a> {
    sys: &'a [u8],
    builder: Builder,
    stats: Stats,
    /// Read so far: bounds can name the same geometry.
    polygons: usize,
}

impl Reader<'_> {
    /// The target of a pointer, with `len` bytes there.
    fn pointer(&self, at: usize, len: usize) -> Result<usize, BnError> {
        let p = self.sys.u64_at(at).ok_or(BnError("a bound is cut off"))?;
        let p =
            p.checked_sub(SYSTEM).and_then(|p| usize::try_from(p).ok()).ok_or(BnError("a pointer leads nowhere"))?;
        let end = p.checked_add(len).filter(|end| *end <= self.sys.len());
        end.map(|_| p).ok_or(BnError("a pointer leads nowhere"))
    }

    fn vec3(&self, at: usize) -> Result<[f32; 3], BnError> {
        let mut v = [0.0; 3];
        for (i, v) in v.iter_mut().enumerate() {
            *v = self.sys.f32_at(at + i * 4).filter(|f| f.is_finite()).ok_or(BnError("a vector is damaged"))?;
        }
        Ok(v)
    }

    fn bound(&mut self, at: usize, m: &Mat4, depth: usize) -> Result<(), BnError> {
        self.stats.bounds += 1;
        if depth > 8 || self.stats.bounds > MAX_BOUNDS {
            return Err(BnError("bounds nested too deeply"));
        }
        match self.sys.get(at + 16) {
            Some(6) => {
                let count = usize::from(self.sys.u16_at(at + 0x98).ok_or(BnError("a bound is cut off"))?);
                if count == 0 {
                    return Ok(());
                }
                let children = self.pointer(at + 0x70, count * 8)?;
                let transforms = match self.sys.u64_at(at + 0x78) {
                    Some(0) => None,
                    _ => Some(self.pointer(at + 0x78, count * 64)?),
                };
                for i in 0..count {
                    if self.sys.u64_at(children + i * 8) == Some(0) {
                        continue;
                    }
                    let child = self.pointer(children + i * 8, 0x70)?;
                    let mut t = math::IDENTITY;
                    if let Some(transforms) = transforms {
                        for (k, v) in t.iter_mut().enumerate() {
                            *v = self.sys.f32_at(transforms + i * 64 + k * 4).unwrap_or(0.0);
                        }
                        (t[3], t[7], t[11], t[15]) = (0.0, 0.0, 0.0, 1.0);
                    }
                    self.bound(child, &math::mul(m, &t), depth + 1)?;
                }
                Ok(())
            }
            Some(5) => self.geometry(at, m),
            Some(_) => {
                self.stats.unknown += 1;
                Ok(())
            }
            None => Err(BnError("a bound is cut off")),
        }
    }

    fn geometry(&mut self, at: usize, m: &Mat4) -> Result<(), BnError> {
        let sys = self.sys;
        let count = |at: usize| sys.u32_at(at).map(|n| n as usize).filter(|n| *n <= 1 << 21);
        let vertex_count = count(at + 0xD0).ok_or(BnError("impossible vertex count"))?;
        let polygon_count = count(at + 0xD4).ok_or(BnError("impossible polygon count"))?;
        if vertex_count == 0 || polygon_count == 0 {
            return Ok(());
        }
        self.polygons += polygon_count;
        if self.polygons > MAX_POLYGONS {
            return Err(BnError("too many polygons"));
        }
        let (quantum, centre) = (self.vec3(at + 0x90)?, self.vec3(at + 0xA0)?);
        let vertices_at = self.pointer(at + 0xB0, vertex_count * 6)?;
        let polygons = self.pointer(at + 0x88, polygon_count * 16)?;
        let vertices: Vec<[f32; 3]> = sys[vertices_at..vertices_at + vertex_count * 6]
            .as_chunks::<6>()
            .0
            .iter()
            .map(|c| {
                std::array::from_fn(|k| {
                    f32::from(i16::from_le_bytes([c[k * 2], c[k * 2 + 1]])) * quantum[k] + centre[k]
                })
            })
            .collect();
        let material_count = usize::from(sys.get(at + 0x120).copied().unwrap_or(0));
        let types = self.pointer(at + 0xF0, material_count * 8).ok();
        let numbers = self.pointer(at + 0x118, polygon_count).ok();
        for (i, p) in sys[polygons..polygons + polygon_count * 16].as_chunks::<16>().0.iter().enumerate() {
            let number = numbers.map_or(0, |n| usize::from(sys[n + i]));
            let kind = types.filter(|_| number < material_count).and_then(|t| sys.u16_at(t + number * 8)).unwrap_or(0);
            let material = self.material(kind);
            let vertex = |at: usize| vertices.get(usize::from(u16::from_le_bytes([p[at], p[at + 1]]))).copied();
            match p[3] {
                0 => {
                    let (Some(a), Some(b), Some(c)) = (vertex(4), vertex(6), vertex(8)) else {
                        return Err(BnError("a triangle names no vertex"));
                    };
                    self.builder.triangle(material, m, [a, b, c]);
                    self.stats.triangles += 1;
                }
                2 | 4 => {
                    let radius = f32::from_le_bytes([p[4], p[5], p[6], p[7]]);
                    let (Some(a), Some(b), true) = (vertex(0), vertex(8), radius.is_finite()) else {
                        return Err(BnError("a capsule names no vertex"));
                    };
                    self.builder.round(material, m, a, b, radius, p[3] == 2);
                    self.stats.primitives += 1;
                }
                _ => self.stats.unknown += 1,
            }
        }
        Ok(())
    }

    /// The number a material type goes by while building (the last one for
    /// all that do not fit).
    fn material(&mut self, kind: u16) -> u8 {
        let known = &mut self.stats.materials;
        let i = known.iter().position(|k| *k == kind).unwrap_or_else(|| {
            if known.len() <= usize::from(u8::MAX) {
                known.push(kind);
            }
            known.len() - 1
        });
        i as u8
    }
}

/// Reads a collision file and turns it into a drawable: one mesh per material
/// type, shader name hash [`MATERIAL_BASE`] + the type.
pub fn parse(system: &[u8]) -> Result<(Drawable, Stats), BnError> {
    if system.u64_at(0) != Some(0) || !matches!(system.get(16), Some(5 | 6)) {
        return Err(BnError("no collision"));
    }
    let mut r = Reader { sys: system, builder: Builder::default(), stats: Stats::default(), polygons: 0 };
    r.bound(0, &math::IDENTITY, 0)?;
    let mut d = Drawable { name: "collision".into(), ..Default::default() };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (i, (material, mesh)) in r.builder.meshes.into_iter().enumerate() {
        for p in &mesh.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let kind = r.stats.materials.get(usize::from(material)).copied().unwrap_or(0);
        d.shaders.push(Shader { name_hash: MATERIAL_BASE + u32::from(kind), ..Default::default() });
        d.meshes.push(Mesh { shader: i as u16, ..mesh });
    }
    if lo[0] <= hi[0] {
        (d.bounds_min, d.bounds_max) = (lo, hi);
    }
    Ok((d, r.stats))
}

#[cfg(test)]
mod tests;
