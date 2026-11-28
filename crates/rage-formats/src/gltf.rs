//! Export of drawables as binary glTF 2.0 (`.glb`), with textures embedded as PNG.
//!
//! GTA V is Z-up (x right, y forward); glTF is Y-up (x right, -z forward). The
//! scene's root node converts `(x, y, z) → (x, z, -y)`, so vertex data and part
//! placements stay in the game's coordinates.

use std::fmt::Write;

use crate::drawable::{Drawable, Mesh, Shader, Skeleton};
use crate::math::{self, Mat4};
use crate::texture::Texture;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// Collects binary data and the JSON fragments describing it.
#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let target = target.map(|t| format!(",\"target\":{t}")).unwrap_or_default();
        self.views.push(format!("{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}{target}}}", bytes.len()));
        self.views.len() - 1
    }

    fn floats<const N: usize>(&mut self, data: &[[f32; N]], kind: &str, bounds: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flatten().flat_map(|v| finite(*v).to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34962));
        let mut extra = String::new();
        if bounds {
            let mut min = [f32::MAX; N];
            let mut max = [f32::MIN; N];
            for v in data {
                for i in 0..N {
                    min[i] = min[i].min(finite(v[i]));
                    max[i] = max[i].max(finite(v[i]));
                }
            }
            let fmt = |a: [f32; N]| a.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(",");
            extra = format!(",\"min\":[{}],\"max\":[{}]", fmt(min), fmt(max));
        }
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":5126,\"count\":{},\"type\":\"{kind}\"{extra}}}",
            data.len()
        ));
        self.accessors.len() - 1
    }

    fn joints(&mut self, data: &[[u16; 4]]) -> usize {
        let bytes: Vec<u8> = data.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        self.skin_accessor(&bytes, data.len(), "VEC4", 5123, Some(34962))
    }

    fn inverse_bind_matrices(&mut self, skeleton: &Skeleton) -> usize {
        let transforms = skeleton.world_transforms();
        let bytes: Vec<u8> =
            transforms.iter().flat_map(math::inverse_affine).map(finite).flat_map(f32::to_le_bytes).collect();
        self.skin_accessor(&bytes, transforms.len(), "MAT4", 5126, None)
    }

    fn skin_accessor(&mut self, bytes: &[u8], count: usize, kind: &str, component: u32, target: Option<u32>) -> usize {
        let view = self.view(bytes, target);
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":{component},\"count\":{count},\"type\":\"{kind}\"}}",
        ));
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}",
            data.len()
        ));
        self.accessors.len() - 1
    }
}

/// A placed mesh in a scene.
pub struct Node {
    pub name: String,
    /// Index into the scene's meshes.
    pub mesh: usize,
    /// Placement in the game's coordinates (Z-up).
    pub transform: Mat4,
}

/// Game Z-up → glTF Y-up: `(x, y, z) → (x, z, -y)`, applied by the root node.
const Z_UP_TO_Y_UP: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

fn matrix_json(m: &Mat4) -> String {
    m.iter().map(|v| format!("{}", finite(*v))).collect::<Vec<_>>().join(",")
}

/// A mesh can be bound when all its vertices have usable, resolved influences.
pub fn has_skin_weights(mesh: &Mesh, bones: usize) -> bool {
    !mesh.positions.is_empty()
        && mesh.bone_tags.is_empty()
        && mesh.bone_hashes.is_empty()
        && mesh.bone_indices.len() == mesh.positions.len()
        && mesh.bone_weights.len() == mesh.positions.len()
        && mesh
            .bone_indices
            .iter()
            .zip(&mesh.bone_weights)
            .all(|(joints, weights)| usable_influences(joints, weights, bones))
}

/// A vertex's influences: weights that add up, each on a bone the skeleton has.
fn usable_influences(joints: &[u16; 4], weights: &[f32; 4], bones: usize) -> bool {
    let sum: f32 = weights.iter().sum();
    sum.is_finite()
        && sum > 0.0
        && joints.iter().zip(weights).all(|(&joint, &weight)| {
            weight.is_finite() && weight >= 0.0 && (weight == 0.0 || usize::from(joint) < bones)
        })
}

fn joint_nodes(skeleton: &Skeleton, first_joint: usize) -> (Vec<usize>, Vec<String>) {
    let mut roots = Vec::new();
    let mut children = vec![Vec::new(); skeleton.bones.len()];
    for (i, bone) in skeleton.bones.iter().enumerate() {
        if let Some(parent) = usize::try_from(bone.parent).ok().filter(|&parent| parent < i) {
            children[parent].push((first_joint + i).to_string());
        } else {
            roots.push(first_joint + i);
        }
    }
    let nodes = skeleton
        .bones
        .iter()
        .zip(children)
        .map(|(bone, children)| {
            let name = esc(&bone.name);
            let local = math::from_trs(bone.translation, bone.rotation, bone.scale);
            let children =
                if children.is_empty() { String::new() } else { format!(",\"children\":[{}]", children.join(",")) };
            format!("{{\"name\":\"{name}\",\"matrix\":[{}]{children}}}", matrix_json(&local))
        })
        .collect();
    (roots, nodes)
}

/// Writes a single drawable as `.glb`.
pub fn write_glb<'a>(d: &Drawable, name: &str, lookup: &dyn Fn(&str) -> Option<&'a Texture>) -> Vec<u8> {
    let node = Node { name: name.to_owned(), mesh: 0, transform: crate::math::IDENTITY };
    write_glb_scene_with_skins(name, &[(name, d)], &[node], &|_, t| lookup(t), &|_, _| Some([1.0; 4]), &|_| {
        d.skeleton.as_ref()
    })
}

/// Writes drawables placed by `nodes` as one `.glb` scene. `lookup` resolves
/// texture names per drawable (embedded dictionary, a `.ytd` next to the
/// model, …); missing textures leave the material untextured. Drawables no
/// node uses are left out. `material` gives a shader's colour factor per
/// drawable and shader index, `None` to leave its meshes out.
pub fn write_glb_scene<'a>(
    name: &str,
    drawables: &[(&str, &Drawable)],
    nodes: &[Node],
    lookup: &dyn Fn(usize, &str) -> Option<&'a Texture>,
    material: &dyn Fn(usize, usize) -> Option<[f32; 4]>,
) -> Vec<u8> {
    write_glb_scene_with_skins(name, drawables, nodes, lookup, material, &|_| None)
}

/// Writes a model scene, resolving the skeleton that each drawable's indices refer to.
pub fn write_glb_scene_with_skins<'a, 's>(
    name: &str,
    drawables: &[(&str, &Drawable)],
    nodes: &[Node],
    lookup: &dyn Fn(usize, &str) -> Option<&'a Texture>,
    material: &dyn Fn(usize, usize) -> Option<[f32; 4]>,
    skeleton: &dyn Fn(usize) -> Option<&'s Skeleton>,
) -> Vec<u8> {
    let inputs = Inputs { lookup, material };
    let mut parts = SceneParts::default();
    let mut meshes_json = Vec::new();
    let mut mesh_of = vec![[None; 2]; drawables.len()];
    let mut binds = vec![None; drawables.len()];

    for (di, (dname, d)) in drawables.iter().enumerate() {
        if !nodes.iter().any(|n| n.mesh == di) {
            continue;
        }
        let first_material = parts.materials.len();
        for (si, shader) in d.shaders.iter().enumerate() {
            let json = parts.material_json(&inputs, (di, dname), (si, shader));
            parts.materials.push(json);
        }
        let rig = skeleton(di).filter(|rig| !rig.bones.is_empty());
        let primitives = parts.primitives(&inputs, di, d, first_material, rig);
        for (group, primitives) in primitives.iter().enumerate().filter(|(_, p)| !p.is_empty()) {
            mesh_of[di][group] = Some(meshes_json.len());
            meshes_json.push(format!("{{\"name\":\"{}\",\"primitives\":[{}]}}", esc(dname), primitives.join(",")));
        }
        if let Some(rig) = rig.filter(|_| mesh_of[di][1].is_some()) {
            binds[di] = Some((rig, parts.b.inverse_bind_matrices(rig)));
        }
    }

    let (nodes_json, children) = placement_nodes(nodes, &mesh_of, &binds, &mut parts.skins);
    let root = format!(
        "{{\"name\":\"{}\",\"matrix\":[{}],\"children\":[{}]}}",
        esc(name),
        matrix_json(&Z_UP_TO_Y_UP),
        children.join(",")
    );
    let json = parts.scene_json(&root, &nodes_json, &meshes_json);
    glb_container(&json, &parts.b.bin)
}

/// The scene's nodes below the root (node 0), and which of them are its children.
fn placement_nodes(
    nodes: &[Node],
    mesh_of: &[[Option<usize>; 2]],
    binds: &[Option<(&Skeleton, usize)>],
    skins: &mut Vec<String>,
) -> (Vec<String>, Vec<String>) {
    let mut nodes_json = Vec::new();
    let mut children = Vec::new();
    for n in nodes {
        let Some(&[rigid, skinned]) = mesh_of.get(n.mesh) else { continue };
        if rigid.is_none() && skinned.is_none() {
            continue;
        }
        children.push((nodes_json.len() + 1).to_string());
        let mesh = rigid.map(|i| format!(",\"mesh\":{i}")).unwrap_or_default();
        let placement = format!("\"name\":\"{}\",\"matrix\":[{}]{mesh}", esc(&n.name), matrix_json(&n.transform));
        if let (Some(mesh), Some((rig, inverse_bind))) = (skinned, binds[n.mesh]) {
            push_skinned_nodes(&mut nodes_json, skins, placement, &n.name, mesh, rig, inverse_bind);
        } else {
            nodes_json.push(format!("{{{placement}}}"));
        }
    }
    (nodes_json, children)
}

/// The nodes of a placement whose mesh `mesh` is bound to a skeleton: the
/// placement, below it the skinned mesh and the skeleton's joints.
fn push_skinned_nodes(
    nodes_json: &mut Vec<String>,
    skins: &mut Vec<String>,
    mut placement: String,
    name: &str,
    mesh: usize,
    rig: &Skeleton,
    inverse_bind: usize,
) {
    let mesh_node = nodes_json.len() + 2;
    let first_joint = mesh_node + 1;
    let (roots, bone_nodes) = joint_nodes(rig, first_joint);
    let roots = std::iter::once(mesh_node).chain(roots).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let _ = write!(placement, ",\"children\":[{roots}]");
    let joints = (first_joint..first_joint + rig.bones.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    skins.push(format!("{{\"joints\":[{joints}],\"inverseBindMatrices\":{inverse_bind}}}"));
    nodes_json.push(format!("{{{placement}}}"));
    nodes_json.push(format!("{{\"name\":\"{}\",\"mesh\":{mesh},\"skin\":{}}}", esc(name), skins.len() - 1));
    nodes_json.extend(bone_nodes);
}

/// What the caller of [`write_glb_scene`] provides: texture and material lookups.
struct Inputs<'s, 'a> {
    lookup: &'s dyn Fn(usize, &str) -> Option<&'a Texture>,
    material: &'s dyn Fn(usize, usize) -> Option<[f32; 4]>,
}

/// The binary data and the JSON fragments of the scene's materials and textures.
#[derive(Default)]
struct SceneParts {
    b: Builder,
    materials: Vec<String>,
    images: Vec<String>,
    textures: Vec<String>,
    skins: Vec<String>,
    /// Texture index per image name.
    image_of: Vec<(String, usize)>,
}

impl SceneParts {
    /// The material of shader `si` of drawable `di`.
    fn material_json(&mut self, inputs: &Inputs, (di, dname): (usize, &str), (si, shader): (usize, &Shader)) -> String {
        let tex_name = shader.base_texture().map(str::to_owned);
        let mut pbr = String::from("\"metallicFactor\":0,\"roughnessFactor\":0.8");
        if let Some(tint) = (inputs.material)(di, si).filter(|t| *t != [1.0; 4]) {
            let [r, g, b, a] = tint.map(finite);
            pbr.push_str(&format!(",\"baseColorFactor\":[{r},{g},{b},{a}]"));
        }
        if let Some(tn) = &tex_name
            && let Some(ti) = self.texture_index(inputs, di, tn)
        {
            pbr.push_str(&format!(",\"baseColorTexture\":{{\"index\":{ti}}}"));
        }
        let label = tex_name.unwrap_or_else(|| format!("{dname}_shader_{si}"));
        // Skin shaders keep other data in the texture's alpha.
        let alpha = if crate::shaders::alpha_is_not_opacity(shader.name_hash) {
            "\"alphaMode\":\"OPAQUE\""
        } else {
            "\"alphaMode\":\"MASK\",\"alphaCutoff\":0.3"
        };
        format!("{{\"name\":\"{}\",\"pbrMetallicRoughness\":{{{pbr}}},{alpha},\"doubleSided\":true}}", esc(&label))
    }

    /// The texture `tn` of drawable `di`, embedded on first use.
    fn texture_index(&mut self, inputs: &Inputs, di: usize, tn: &str) -> Option<usize> {
        let existing = self.image_of.iter().find(|(n, _)| n.eq_ignore_ascii_case(tn)).map(|(_, i)| *i);
        existing.or_else(|| self.embed_texture(inputs, di, tn))
    }

    fn embed_texture(&mut self, inputs: &Inputs, di: usize, tn: &str) -> Option<usize> {
        let t = (inputs.lookup)(di, tn)?;
        let rgba = t.to_rgba()?;
        let png = crate::png(t.width, t.height, &rgba).ok()?;
        let view = self.b.view(&png, None);
        self.images.push(format!("{{\"name\":\"{}\",\"bufferView\":{view},\"mimeType\":\"image/png\"}}", esc(tn)));
        self.textures.push(format!("{{\"source\":{},\"sampler\":0}}", self.images.len() - 1));
        self.image_of.push((tn.to_owned(), self.textures.len() - 1));
        Some(self.textures.len() - 1)
    }

    /// The rigid and skinned primitives of drawable `di` that are drawn.
    fn primitives(
        &mut self,
        inputs: &Inputs,
        di: usize,
        d: &Drawable,
        first_material: usize,
        rig: Option<&Skeleton>,
    ) -> [Vec<String>; 2] {
        let mut primitives = [Vec::new(), Vec::new()];
        for mesh in &d.meshes {
            if mesh.positions.is_empty()
                || mesh.indices.is_empty()
                || (inputs.material)(di, mesh.shader as usize).is_none()
            {
                continue;
            }
            let material = ((mesh.shader as usize) < d.shaders.len()).then(|| first_material + mesh.shader as usize);
            let skinned = rig.is_some_and(|rig| has_skin_weights(mesh, rig.bones.len()));
            primitives[usize::from(skinned)].push(primitive_json(&mut self.b, mesh, material, skinned));
        }
        primitives
    }

    /// The glTF JSON, padded to a multiple of 4 bytes with spaces. Pads the binary data as well.
    fn scene_json(&mut self, root: &str, nodes_json: &[String], meshes_json: &[String]) -> Vec<u8> {
        let mut json = String::new();
        let _ = write!(
            json,
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"OpenRAGE\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\
             \"nodes\":[{root}{}{}],\"meshes\":[{}]",
            if nodes_json.is_empty() { "" } else { "," },
            nodes_json.join(","),
            meshes_json.join(",")
        );
        if !self.skins.is_empty() {
            let _ = write!(json, ",\"skins\":[{}]", self.skins.join(","));
        }
        if !self.materials.is_empty() {
            let _ = write!(json, ",\"materials\":[{}]", self.materials.join(","));
        }
        if !self.textures.is_empty() {
            let _ = write!(
                json,
                ",\"textures\":[{}],\"images\":[{}],\"samplers\":[{{\"magFilter\":9729,\"minFilter\":9987,\"wrapS\":10497,\"wrapT\":10497}}]",
                self.textures.join(","),
                self.images.join(",")
            );
        }
        while !self.b.bin.len().is_multiple_of(4) {
            self.b.bin.push(0);
        }
        let _ = write!(
            json,
            ",\"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{}}}]}}",
            self.b.accessors.join(","),
            self.b.views.join(","),
            self.b.bin.len()
        );
        let mut json = json.into_bytes();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        json
    }
}

/// One primitive of `mesh`: its attributes and indices go to `b`.
fn primitive_json(b: &mut Builder, mesh: &Mesh, material: Option<usize>, skinned: bool) -> String {
    let mut attrs = format!("\"POSITION\":{}", b.floats(&mesh.positions, "VEC3", true));
    if mesh.normals.len() == mesh.positions.len() {
        attrs.push_str(&format!(",\"NORMAL\":{}", b.floats(&unit_normals(&mesh.normals), "VEC3", false)));
    }
    if mesh.uvs.len() == mesh.positions.len() {
        attrs.push_str(&format!(",\"TEXCOORD_0\":{}", b.floats(&mesh.uvs, "VEC2", false)));
    }
    if mesh.colors.len() == mesh.positions.len() {
        attrs.push_str(&format!(",\"COLOR_0\":{}", b.floats(&mesh.colors, "VEC4", false)));
    }
    if skinned {
        let (joints, weights) = normalized_skin(mesh);
        let joints = b.joints(&joints);
        let weights = b.floats(&weights, "VEC4", false);
        let _ = write!(attrs, ",\"JOINTS_0\":{joints},\"WEIGHTS_0\":{weights}");
    }
    let idx = b.indices(&mesh.indices);
    let material = material.map(|m| format!(",\"material\":{m}")).unwrap_or_default();
    format!("{{\"attributes\":{{{attrs}}},\"indices\":{idx}{material}}}")
}

fn normalized_skin(mesh: &Mesh) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
    let mut joints = mesh.bone_indices.clone();
    let mut weights = mesh.bone_weights.clone();
    for (joints, weights) in joints.iter_mut().zip(&mut weights) {
        let sum: f32 = weights.iter().sum();
        // Palette remapping can collapse multiple influences into the same joint.
        for i in 1..joints.len() {
            if let Some(j) = (0..i).find(|&j| joints[j] == joints[i]) {
                weights[j] += weights[i];
                weights[i] = 0.0;
            }
        }
        for (joint, weight) in joints.iter_mut().zip(weights) {
            if *weight == 0.0 {
                *joint = 0;
            }
            *weight /= sum;
        }
    }
    (joints, weights)
}

fn unit_normals(normals: &[[f32; 3]]) -> Vec<[f32; 3]> {
    normals
        .iter()
        .map(|n| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 1e-6 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 0.0, 1.0] }
        })
        .collect()
}

/// The `.glb` file: header, JSON chunk and binary chunk.
fn glb_container(json: &[u8], bin: &[u8]) -> Vec<u8> {
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_glb_structure() {
        let d = Drawable {
            meshes: vec![Mesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                colors: Vec::new(),
                indices: vec![0, 1, 2],
                shader: 0,
                ..Default::default()
            }],
            shaders: vec![Shader { name_hash: 1, textures: vec!["body".into()], samplers: Vec::new() }],
            ..Default::default()
        };
        let glb = write_glb(&d, "tri", &|_| None);
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(json.contains("\"POSITION\"") && json.contains("\"NORMAL\"") && json.contains("\"material\":0"));
        // Vertices stay in game coordinates; the root node converts Z-up → Y-up.
        assert!(json.contains("\"min\":[0,0,0],\"max\":[1,1,0]"), "{json}");
        assert!(json.contains("\"matrix\":[1,0,0,0,0,0,-1,0,0,1,0,0,0,0,0,1]"), "{json}");
    }
    #[test]
    fn writes_skin_and_keeps_unweighted_primitives_rigid() {
        use crate::drawable::Bone;
        let mut rigid = Mesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let mut skinned = rigid.clone();
        rigid.bone_indices = vec![[9, 0, 0, 0]; 3];
        rigid.bone_weights = vec![[1.0, 0.0, 0.0, 0.0]; 3];
        skinned.bone_indices = vec![[0, 1, 1, 9]; 3];
        skinned.bone_weights = vec![[0.25, 0.25, 0.5, 0.0]; 3];
        let bone = |name: &str, parent, translation| Bone {
            name: name.into(),
            parent,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            ..Default::default()
        };
        let d = Drawable {
            meshes: vec![rigid, skinned],
            skeleton: Some(Skeleton { bones: vec![bone("root", -1, [0.0; 3]), bone("tip", 0, [1.0, 0.0, 0.0])] }),
            ..Default::default()
        };
        let glb = write_glb(&d, "tri", &|_| None);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert_eq!(json.matches("\"JOINTS_0\"").count(), 1);
        assert_eq!(json.matches("\"WEIGHTS_0\"").count(), 1);
        assert!(
            json.contains("\"name\":\"root\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],\"children\":[4]"),
            "{json}"
        );
        assert!(json.contains("\"skins\":[{\"joints\":[3,4],\"inverseBindMatrices\":6}]"), "{json}");
        let bin = &glb[28 + json_len..];
        assert_eq!(&bin[84..92], &[0, 0, 1, 0, 0, 0, 0, 0]);
        let get_f32 = |offset| f32::from_le_bytes(bin[offset..offset + 4].try_into().unwrap());
        assert_eq!((get_f32(108), get_f32(112), get_f32(116)), (0.25, 0.75, 0.0));
        assert_eq!(get_f32(280), -1.0);
    }
}
