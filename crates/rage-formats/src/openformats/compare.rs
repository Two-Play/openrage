//! Comparing models, to check that writing and reading text keeps them.

use super::*;

/// Whether a number read back from text is the one written (seven
/// significant digits).
pub(crate) fn close(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a - b).abs() <= 1e-6 + 2e-6 * a.abs().max(b.abs()) || (a.is_nan() && b.is_nan())
}

fn close_all(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| close(*x, *y))
}

fn shader_difference(i: usize, x: &OfShader, y: &OfShader) -> Option<String> {
    if !x.name.eq_ignore_ascii_case(&y.name) || x.params.len() != y.params.len() {
        return Some(format!(
            "shader {i}: {} with {} parameters, read back {} with {}",
            x.name,
            x.params.len(),
            y.name,
            y.params.len()
        ));
    }
    for ((n, v), (m, w)) in x.params.iter().zip(&y.params) {
        let same = n.eq_ignore_ascii_case(m)
            && match (v, w) {
                (OfValue::Texture(s), OfValue::Texture(t)) => s.eq_ignore_ascii_case(t),
                (OfValue::Floats(s), OfValue::Floats(t)) => close_all(s, t),
                _ => false,
            };
        if !same {
            return Some(format!("shader {i}: parameter {n} = {v:?} read back as {m} = {w:?}"));
        }
    }
    None
}

fn shaders_difference(a: &OfDrawable, b: &OfDrawable) -> Option<String> {
    if a.shaders.len() != b.shaders.len() {
        return Some(format!("{} shaders, {} read back", a.shaders.len(), b.shaders.len()));
    }
    a.shaders.iter().zip(&b.shaders).enumerate().find_map(|(i, (x, y))| shader_difference(i, x, y))
}

/// The bone a bone of the model is at in the text's order.
fn remap(rank: &[usize], i: usize) -> usize {
    rank.get(i).copied().unwrap_or(i)
}

fn bone_differs(rank: &[usize], i: usize, x: &OfBone, y: &OfBone) -> bool {
    let parent = usize::try_from(x.parent).ok().filter(|&p| p < i).map_or(-1, |p| remap(rank, p) as i16);
    let same = (x.name.is_empty() && y.name == "unnamed" || x.name == y.name)
        && x.tag == y.tag
        && x.flags & 0xFFF == y.flags
        && parent == y.parent
        && close_all(&x.rotation, &y.rotation)
        && close_all(&x.translation, &y.translation)
        && close_all(&x.scale, &y.scale);
    !same
}

fn skeleton_difference(a: &OfDrawable, b: &OfDrawable, rank: &[usize]) -> Option<String> {
    match (&a.skeleton, &b.skeleton) {
        (None, None) => None,
        (Some(s), Some(t)) => {
            if s.bones.len() != t.bones.len() || s.crc != t.crc {
                return Some(format!("skeleton: {} bones, {} read back", s.bones.len(), t.bones.len()));
            }
            s.bones.iter().enumerate().find_map(|(i, x)| {
                bone_differs(rank, i, x, &t.bones[remap(rank, i)])
                    .then(|| format!("bone {i} ({}) differs after reading back", x.name))
            })
        }
        _ => Some("the skeleton is lost".into()),
    }
}

fn geometry_difference(what: &str, g: &OfGeometry, h: &OfGeometry, rank: &[usize]) -> Option<String> {
    if g.shader != h.shader || !g.decl.same_layout(h.decl) || g.indices != h.indices {
        return Some(format!("{what}: shader, declaration or indices differ"));
    }
    for ((c, v), (d, w)) in g.attributes.iter().zip(&h.attributes) {
        let widths = g.decl.components().find(|(k, _)| k == c).map_or(4, |(k, t)| VertexDecl::width(k, t));
        let same = c == d
            && v.len() == w.len()
            && v.iter().zip(w).all(|(p, q)| {
                let p = if *c == component::BLEND_INDICES { p.map(|b| remap(rank, b as usize) as f32) } else { *p };
                close_all(&p[..widths], &q[..widths])
            });
        if !same {
            return Some(format!("{what}: vertex component {c} differs after reading back"));
        }
    }
    None
}

fn model_difference(level: &str, i: usize, x: &OfModel, y: &OfModel, rank: &[usize]) -> Option<String> {
    if remap(rank, x.bone as usize) != y.bone as usize
        || (x.skinned, x.bone_count, x.mask) != (y.skinned, y.bone_count, y.mask)
        || x.bounds.len() != y.bounds.len()
        || x.geometries.len() != y.geometries.len()
    {
        return Some(format!("{level} model {i}: header differs after reading back"));
    }
    for ((p, q), (r, s)) in x.bounds.iter().zip(&y.bounds) {
        if !close_all(p, r) || !close_all(q, s) {
            return Some(format!("{level} model {i}: bounds differ"));
        }
    }
    x.geometries
        .iter()
        .zip(&y.geometries)
        .enumerate()
        .find_map(|(j, (g, h))| geometry_difference(&format!("{level} model {i} geometry {j}"), g, h, rank))
}

fn lods_difference(a: &OfDrawable, b: &OfDrawable, rank: &[usize]) -> Option<String> {
    for (k, (la, lb)) in a.lods.iter().zip(&b.lods).enumerate() {
        let level = LOD_NAMES[k];
        if !close(la.distance, lb.distance) || la.models.len() != lb.models.len() {
            return Some(format!("{level}: {} models, {} read back", la.models.len(), lb.models.len()));
        }
        let found =
            la.models.iter().zip(&lb.models).enumerate().find_map(|(i, (x, y))| model_difference(level, i, x, y, rank));
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The first difference between a model and the same model written as text
/// and read back (`b`), if any. Bones may be in another order (the text
/// lists them as a tree); blend indices and bone references must follow.
pub fn difference(a: &OfDrawable, b: &OfDrawable) -> Option<String> {
    if let Some(d) = shaders_difference(a, b) {
        return Some(d);
    }
    let rank = a.skeleton.as_ref().map(write::bone_ranks).unwrap_or_default();
    if let Some(d) = skeleton_difference(a, b, &rank) {
        return Some(d);
    }
    if !close_all(&a.center, &b.center)
        || !close(a.radius, b.radius)
        || !close_all(&a.aabb_min, &b.aabb_min)
        || !close_all(&a.aabb_max, &b.aabb_max)
    {
        return Some("bounds differ".into());
    }
    if let Some(d) = lods_difference(a, b, &rank) {
        return Some(d);
    }
    if a.textures.len() != b.textures.len() {
        return Some(format!("{} embedded textures, {} read back", a.textures.len(), b.textures.len()));
    }
    a.textures.iter().zip(&b.textures).find_map(|(x, y)| texture_difference(&x.texture, &y.texture))
}

/// The first difference between a texture and the same texture written as
/// DDS and read back.
pub fn texture_difference(x: &crate::texture::Texture, y: &crate::texture::Texture) -> Option<String> {
    let what = if !write::same_texture_name(&x.name, &y.name) {
        format!("named {}", y.name)
    } else if (x.width, x.height, x.depth, x.levels) != (y.width, y.height, y.depth, y.levels) {
        format!("{}×{}×{} with {} levels", y.width, y.height, y.depth, y.levels)
    } else if x.format != y.format {
        format!("format {} instead of {}", y.format.name(), x.format.name())
    } else if x.dimension != y.dimension {
        format!("{:?} instead of {:?}", y.dimension, x.dimension)
    } else if x.usage != y.usage {
        format!("usage {} instead of {}", y.usage, x.usage)
    } else if x.data != y.data {
        format!("{} bytes of pixels instead of {}", y.data.len(), x.data.len())
    } else {
        return None;
    };
    Some(format!("texture {} differs after reading back: {what}", x.name))
}
