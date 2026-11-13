//! Reading Wavefront OBJ models for import, and fitting them to a drawable's
//! shaders.
//!
//! OBJ is usually Y-up with texture coordinates growing upwards; the game is
//! Z-up with DirectX coordinates, so `(x, y, z) → (x, −z, y)` and `v → 1 − v`.

use std::collections::HashMap;

use anyhow::{Result, bail};
use rage_formats::ydr_write::{MAX_VERTICES, NewMesh};

/// Triangles of one material.
#[derive(Debug, Clone, Default)]
pub struct ObjMesh {
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Reads an OBJ file: `v`, `vt`, `vn`, `f` (polygons fanned into triangles,
/// negative indices from the end) and `usemtl`. One mesh per material, in the
/// order they first appear.
pub fn read(text: &str) -> Result<Vec<ObjMesh>> {
    let (mut v, mut vt, mut vn) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 2]>::new(), Vec::<[f32; 3]>::new());
    let mut meshes: Vec<ObjMesh> = Vec::new();
    let mut seen: Vec<HashMap<(usize, usize, usize), u32>> = Vec::new();
    let mut current = 0usize;
    let mut has_uv = false;
    let mut has_normals = false;
    let num = |s: Option<&str>, line: usize| -> Result<f32> {
        match s.map(str::parse::<f32>) {
            Some(Ok(x)) => Ok(x),
            _ => bail!("line {line}: number missing or invalid"),
        }
    };
    let index = |s: &str, len: usize, line: usize| -> Result<usize> {
        let k: i64 = s.parse().map_err(|_| anyhow::anyhow!("line {line}: bad index {s:?}"))?;
        let k = if k < 0 { len as i64 + k } else { k - 1 };
        if k < 0 || k as usize >= len {
            bail!("line {line}: index {s} out of range");
        }
        Ok(k as usize)
    };
    let mesh_for = |name: &str, meshes: &mut Vec<ObjMesh>, seen: &mut Vec<HashMap<_, u32>>| -> usize {
        match meshes.iter().position(|m| m.material == name) {
            Some(i) => i,
            None => {
                meshes.push(ObjMesh { material: name.to_owned(), ..Default::default() });
                seen.push(HashMap::new());
                meshes.len() - 1
            }
        }
    };
    let mut material = String::from("default");
    let mut started = false;
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let mut it = raw.split_whitespace();
        match it.next() {
            Some("v") => {
                let (x, y, z) = (num(it.next(), line)?, num(it.next(), line)?, num(it.next(), line)?);
                v.push([x, -z, y]);
            }
            Some("vt") => {
                let u = num(it.next(), line)?;
                let t = it.next().map_or(Ok(0.0), |s| num(Some(s), line))?;
                vt.push([u, 1.0 - t]);
            }
            Some("vn") => {
                let (x, y, z) = (num(it.next(), line)?, num(it.next(), line)?, num(it.next(), line)?);
                vn.push([x, -z, y]);
            }
            Some("usemtl") => {
                material = it.collect::<Vec<_>>().join(" ");
                started = false;
            }
            Some("f") => {
                if !started {
                    current = mesh_for(&material, &mut meshes, &mut seen);
                    started = true;
                }
                let mut corners = Vec::new();
                for corner in it {
                    let mut f = corner.split('/');
                    let p = index(f.next().unwrap_or(""), v.len(), line)?;
                    let t = match f.next() {
                        Some(s) if !s.is_empty() => {
                            has_uv = true;
                            index(s, vt.len(), line)? + 1
                        }
                        _ => 0,
                    };
                    let nn = match f.next() {
                        Some(s) if !s.is_empty() => {
                            has_normals = true;
                            index(s, vn.len(), line)? + 1
                        }
                        _ => 0,
                    };
                    let mesh = &mut meshes[current];
                    let key = (p, t, nn);
                    let i = match seen[current].get(&key) {
                        Some(&i) => i,
                        None => {
                            let i = mesh.positions.len() as u32;
                            mesh.positions.push(v[p]);
                            mesh.uvs.push(if t > 0 { vt[t - 1] } else { [0.0, 0.0] });
                            mesh.normals.push(if nn > 0 { vn[nn - 1] } else { [0.0, 0.0, 0.0] });
                            seen[current].insert(key, i);
                            i
                        }
                    };
                    corners.push(i);
                }
                if corners.len() < 3 {
                    bail!("line {line}: a face needs at least three corners");
                }
                for k in 1..corners.len() - 1 {
                    meshes[current].indices.extend([corners[0], corners[k], corners[k + 1]]);
                }
            }
            _ => {}
        }
    }
    meshes.retain(|m| !m.indices.is_empty());
    if meshes.is_empty() {
        bail!("the OBJ file has no faces");
    }
    for m in &mut meshes {
        if !has_uv {
            m.uvs.clear();
        }
        // Normals only when every corner had one; otherwise they are computed.
        if !has_normals || m.normals.contains(&[0.0; 3]) {
            m.normals.clear();
        }
    }
    Ok(meshes)
}

/// Splits a mesh into pieces of at most [`MAX_VERTICES`] vertices.
fn split(m: NewMesh) -> Vec<NewMesh> {
    if m.positions.len() <= MAX_VERTICES {
        return vec![m];
    }
    let mut out = Vec::new();
    let mut piece = NewMesh { shader: m.shader, ..Default::default() };
    let mut map: HashMap<u32, u32> = HashMap::new();
    for t in m.indices.as_chunks::<3>().0 {
        let new = t.iter().filter(|i| !map.contains_key(i)).count();
        if piece.positions.len() + new > MAX_VERTICES {
            out.push(std::mem::replace(&mut piece, NewMesh { shader: m.shader, ..Default::default() }));
            map.clear();
        }
        for &i in t {
            let k = *map.entry(i).or_insert_with(|| {
                let k = piece.positions.len() as u32;
                piece.positions.push(m.positions[i as usize]);
                if !m.normals.is_empty() {
                    piece.normals.push(m.normals[i as usize]);
                }
                if !m.uvs.is_empty() {
                    piece.uvs.push(m.uvs[i as usize]);
                }
                if !m.colors.is_empty() {
                    piece.colors.push(m.colors[i as usize]);
                }
                k
            });
            piece.indices.push(k);
        }
    }
    if !piece.indices.is_empty() {
        out.push(piece);
    }
    out
}

/// Meshes for the writer, and per OBJ material the shader it went to.
pub type Fitted = (Vec<NewMesh>, Vec<(String, u16)>);

/// Meshes for the model writer: the OBJ materials in order go to the
/// template's shaders in order (`shader` puts everything on one); the last
/// shader takes the materials left over. Returns the meshes and, per OBJ
/// material, the shader it went to.
pub fn fit(meshes: Vec<ObjMesh>, shaders: &[u16], shader: Option<u16>) -> Result<Fitted> {
    let Some(&last) = shaders.last() else { bail!("the template has no shader meshes can use") };
    if let Some(s) = shader
        && !shaders.contains(&s)
    {
        bail!("shader {s} cannot be used; the template has {shaders:?}");
    }
    let mut out = Vec::new();
    let mut mapping = Vec::new();
    for (i, m) in meshes.into_iter().enumerate() {
        let s = shader.unwrap_or_else(|| shaders.get(i).copied().unwrap_or(last));
        mapping.push((m.material.clone(), s));
        out.extend(split(NewMesh {
            shader: s,
            positions: m.positions,
            normals: m.normals,
            uvs: m.uvs,
            colors: Vec::new(),
            indices: m.indices,
        }));
    }
    Ok((out, mapping))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_materials_and_converts_axes() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 1\nusemtl a\nf 1/1 2/2 3/2 4/1\nusemtl b\nf 1 2 3\nusemtl a\nf 2 3 4\n";
        let m = read(obj).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].material, "a");
        assert_eq!(m[0].indices.len(), 9, "both 'a' groups in one mesh");
        assert_eq!(m[0].positions[2], [1.0, 0.0, 1.0], "Y-up → Z-up");
        assert_eq!(m[0].uvs[0], [0.0, 1.0], "v flipped");
        assert!(m[0].normals.is_empty(), "no normals given: computed later");
        assert!(read("v 0 0 0\nf 1 2 3").is_err());
    }

    #[test]
    fn fits_materials_to_shaders_and_splits_big_meshes() {
        let mesh = |name: &str| ObjMesh {
            material: name.into(),
            positions: vec![[0.0; 3]; 3],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2],
        };
        let (meshes, mapping) = fit(vec![mesh("a"), mesh("b"), mesh("c")], &[0, 2], None).unwrap();
        assert_eq!(mapping, [("a".to_owned(), 0), ("b".to_owned(), 2), ("c".to_owned(), 2)]);
        assert_eq!(meshes.len(), 3);
        assert!(fit(vec![mesh("a")], &[0], Some(5)).is_err());

        // 30,000 separate triangles (90,000 vertices): two pieces.
        let n = 30_000 * 3;
        let big =
            NewMesh { shader: 0, positions: vec![[0.0; 3]; n], indices: (0..n as u32).collect(), ..Default::default() };
        let pieces = split(big);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| p.positions.len() <= MAX_VERTICES));
        assert_eq!(pieces.iter().map(|p| p.indices.len()).sum::<usize>(), n);
    }
}
