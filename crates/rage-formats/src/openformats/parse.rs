//! Reading openFormats text files back into the model.

use rage_resource::ResourceError;

use super::text::{self, Node};
use super::*;
use crate::drawable::bad;

/// Reads a file of an export by its path (`/`-separated, relative to the
/// folder the export was written to).
pub type Source<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, ResourceError>;

/// `dir` joined with a path as written in a file (`\` or `/`), `..` resolved.
pub(crate) fn resolve(dir: &str, path: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in path.split(['\\', '/']) {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn stem_of(path: &str) -> String {
    let name = path.rsplit_once('/').map_or(path, |(_, n)| n);
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_owned()
}

fn document(src: Source, path: &str) -> Result<text::Document, ResourceError> {
    let data = src(path)?;
    let text = String::from_utf8_lossy(&data);
    text::parse(&text).map_err(|e| bad(format!("{path}: {e}")))
}

fn in_file<T>(path: &str, r: Result<T, ResourceError>) -> Result<T, ResourceError> {
    r.map_err(|e| match e {
        ResourceError::Format(m) if !m.starts_with(path) => bad(format!("{path}: {m}")),
        e => e,
    })
}

/// A drawable from its `.odr` file.
pub fn parse_odr(src: Source, path: &str) -> Result<OfDrawable, ResourceError> {
    let doc = document(src, path)?;
    let mut d = in_file(path, drawable(src, dir_of(path), &doc.root))?;
    d.name = stem_of(path);
    Ok(d)
}

/// A fragment from its `.oft` file.
pub fn parse_oft(src: Source, path: &str) -> Result<OfFragment, ResourceError> {
    let doc = document(src, path)?;
    let dir = dir_of(path);
    let main = doc.root.get("Drawable").ok_or_else(|| bad(format!("{path}: no Drawable block")))?;
    let mut f = OfFragment { drawable: in_file(path, drawable(src, dir, main))?, children: Vec::new() };
    f.drawable.name = stem_of(path);
    if let Some(children) = doc.root.get("Children") {
        for (i, c) in children.all("Child").enumerate() {
            let tag = in_file(path, c.get("BoneTag").ok_or_else(|| bad("a Child without BoneTag")))?;
            let tag = in_file(path, tag.int())?;
            let block = c.get("Drawable").ok_or_else(|| bad(format!("{path}: a Child without Drawable")))?;
            let mut d = in_file(path, drawable(src, dir, block))?;
            d.name = format!("{}_child{i}", f.drawable.name);
            f.children.push((tag, d));
        }
    }
    Ok(f)
}

/// The drawables of a drawable dictionary from its `.odd` file.
pub fn parse_odd(src: Source, path: &str) -> Result<Vec<OfDrawable>, ResourceError> {
    let doc = document(src, path)?;
    let mut out = Vec::new();
    for n in doc.root.children() {
        if n.key().to_ascii_lowercase().ends_with(".odr") {
            out.push(parse_odr(src, &resolve(dir_of(path), n.key()))?);
        }
    }
    Ok(out)
}

/// The textures of a texture dictionary from its `.otd` file.
pub fn parse_otd(src: Source, path: &str) -> Result<Vec<OfTexture>, ResourceError> {
    let doc = document(src, path)?;
    let mut out = Vec::new();
    for n in doc.root.children() {
        let otx = resolve(dir_of(path), n.key());
        let x = document(src, &otx)?;
        let image = x.root.get("Image").ok_or_else(|| bad(format!("{otx}: no Image")))?;
        let dds = resolve(dir_of(&otx), image.arg(0)?);
        let mut texture = crate::dds::from_dds(&name_of_file_stem(&stem_of(&otx)), &src(&dds)?)
            .map_err(|e| bad(format!("{dds}: {e}")))?;
        if let Some(usage) = x.root.get("Usage").and_then(|u| u.args().first())
            && let Some(code) = usage_code(usage)
        {
            texture.usage = code;
        }
        out.push(OfTexture { texture });
    }
    Ok(out)
}

fn drawable(src: Source, dir: &str, node: &Node) -> Result<OfDrawable, ResourceError> {
    let mut d = OfDrawable::default();
    if let Some(shaders) = node.get("Shaders") {
        d.shaders.extend(shaders.children().iter().map(shader));
    }
    if let Some(p) = linked_path(node, "TextureDictionary") {
        d.textures = parse_otd(src, &resolve(dir, p))?;
    }
    if let Some(p) = linked_path(node, "Skeleton") {
        d.skeleton = Some(skeleton_file(src, &resolve(dir, p))?);
    }
    let lods = node.get("LodGroup").ok_or_else(|| bad("no LodGroup"))?;
    read_lods(src, dir, lods, &mut d)?;
    read_bounds(lods, &mut d)?;
    Ok(d)
}

fn skeleton_file(src: Source, path: &str) -> Result<OfSkeleton, ResourceError> {
    let doc = document(src, path)?;
    in_file(path, skeleton(&doc.root))
}

fn read_lods(src: Source, dir: &str, lods: &Node, d: &mut OfDrawable) -> Result<(), ResourceError> {
    for (k, name) in LOD_NAMES.iter().enumerate() {
        let Some(level) = lods.get(name) else { continue };
        if let Some(w) = level.args().first() {
            d.lods[k].distance = text::parse_f32(w, level.line)?;
        }
        for m in level.children() {
            if let Some(model) = lod_model(src, dir, m)? {
                d.lods[k].models.push(model);
            }
        }
    }
    Ok(())
}

fn read_bounds(lods: &Node, d: &mut OfDrawable) -> Result<(), ResourceError> {
    let v3 = |key: &str| lods.get(key).map(Node::vec3).transpose();
    d.center = v3("Center")?.unwrap_or_default();
    d.aabb_min = v3("AABBMin")?.unwrap_or_default();
    d.aabb_max = v3("AABBMax")?.unwrap_or_default();
    d.radius = lods.get("Radius").map(Node::float).transpose()?.unwrap_or_default();
    Ok(())
}

fn shader(s: &Node) -> OfShader {
    let name = s.key();
    let name = name.strip_suffix(".sps").or_else(|| name.strip_suffix(".SPS")).unwrap_or(name);
    OfShader { name: name.to_owned(), params: s.children().iter().map(param).collect() }
}

/// The file a block links to in its first argument, unless it is `null`.
fn linked_path<'a>(node: &'a Node, key: &str) -> Option<&'a String> {
    node.get(key)?.args().first().filter(|p| !p.eq_ignore_ascii_case("null"))
}

/// The model of one entry of a LOD level; `None` for the `none`/`null` placeholders.
fn lod_model(src: Source, dir: &str, m: &Node) -> Result<Option<OfModel>, ResourceError> {
    if m.key().eq_ignore_ascii_case("none") || m.key().eq_ignore_ascii_case("null") {
        return Ok(None);
    }
    let path = resolve(dir, m.key());
    let doc = document(src, &path)?;
    let mut model = in_file(&path, mesh(&doc.root))?;
    model.bone = match m.args().first() {
        Some(b) => b.parse().map_err(|_| bad(format!("line {}: bad bone index {b}", m.line)))?,
        None => 0,
    };
    Ok(Some(model))
}

fn param(n: &Node) -> (String, OfValue) {
    let name = n.key().to_owned();
    let texture = || {
        let t = n.args().join(" ");
        OfValue::Texture(if t.eq_ignore_ascii_case("null") || t == "*NULL*" { String::new() } else { t })
    };
    let value = match param_name(param_hash(&name)).1 {
        Some(0) => texture(),
        _ => match n.floats() {
            Ok(v) if !v.is_empty() => OfValue::Floats(v),
            _ => texture(),
        },
    };
    (name, value)
}

fn skeleton(root: &Node) -> Result<OfSkeleton, ResourceError> {
    let mut s = OfSkeleton { crc: root.get("DataCRC").map(Node::int).transpose()?.unwrap_or(0), bones: Vec::new() };
    fn bone(s: &mut OfSkeleton, n: &Node, parent: i16) -> Result<(), ResourceError> {
        let index = s.bones.len();
        let tag =
            n.args().get(1).map(|t| t.parse()).transpose().map_err(|_| bad(format!("line {}: bad tag", n.line)))?;
        let v = |key: &str, len: usize, default: &[f32]| -> Result<Vec<f32>, ResourceError> {
            match n.get(key) {
                Some(k) => {
                    let v = k.floats()?;
                    if v.len() < len {
                        return Err(bad(format!("line {}: {key} needs {len} numbers", k.line)));
                    }
                    Ok(v)
                }
                None => Ok(default.to_vec()),
            }
        };
        let (r, t, sc) = (
            v("RotationQuaternion", 4, &[0.0, 0.0, 0.0, 1.0])?,
            v("LocalOffset", 3, &[0.0; 3])?,
            v("Scale", 3, &[1.0; 3])?,
        );
        let flags = n.get("Flags").map_or(0, |f| {
            f.args()
                .iter()
                .filter_map(|w| BONE_FLAGS.iter().find(|(_, n)| n.eq_ignore_ascii_case(w)))
                .fold(0, |a, (b, _)| a | b)
        });
        let tag = tag.unwrap_or(0);
        s.bones.push(OfBone {
            name: n.arg(0)?.to_owned(),
            tag,
            mirror: n.get("MirrorBoneId").map(Node::int).transpose()?.unwrap_or(tag),
            parent,
            flags,
            rotation: [r[0], r[1], r[2], r[3]],
            translation: [t[0], t[1], t[2]],
            scale: [sc[0], sc[1], sc[2]],
        });
        if let Some(children) = n.children().iter().find(|c| c.key().eq_ignore_ascii_case("Children")) {
            for c in children.all("Bone") {
                bone(s, c, index as i16)?;
            }
        }
        Ok(())
    }
    for b in root.all("Bone") {
        bone(&mut s, b, -1)?;
    }
    Ok(s)
}

fn mesh(root: &Node) -> Result<OfModel, ResourceError> {
    let mut m = OfModel {
        skinned: root.get("Skinned").map(Node::bool).transpose()?.unwrap_or(false),
        bone_count: root.get("BoneCount").map(Node::int).transpose()?.unwrap_or(0),
        mask: root.get("Mask").map(Node::int).transpose()?.unwrap_or(0xFF),
        ..Default::default()
    };
    if let Some(bounds) = root.get("Bounds") {
        for b in bounds.all("Aabb") {
            let get = |k: &str| b.get(k).ok_or_else(|| bad(format!("line {}: Aabb without {k}", b.line)))?.vec3();
            m.bounds.push((get("Min")?, get("Max")?));
        }
    }
    let geometries = root.get("Geometries").ok_or_else(|| bad("no Geometries"))?;
    for g in geometries.all("Geometry") {
        m.geometries.push(geometry(g)?);
    }
    Ok(m)
}

fn geometry(g: &Node) -> Result<OfGeometry, ResourceError> {
    let shader = g.get("ShaderIndex").ok_or_else(|| bad(format!("line {}: no ShaderIndex", g.line)))?.int()?;
    let code = g.get("VertexDeclaration").ok_or_else(|| bad(format!("line {}: no VertexDeclaration", g.line)))?;
    let decl = vertex_decl(code)?;
    let indices_node = g.get("Indices").ok_or_else(|| bad(format!("line {}: no Indices", g.line)))?;
    let indices = geometry_indices(indices_node)?;
    let vertices = g.get("Vertices").ok_or_else(|| bad(format!("line {}: no Vertices", g.line)))?;
    let attributes = geometry_attributes(vertices, &decl)?;
    check_indices(&indices, vertices.children().len(), indices_node.line)?;
    Ok(OfGeometry { shader, decl, indices, attributes })
}

fn vertex_decl(code: &Node) -> Result<VertexDecl, ResourceError> {
    VertexDecl::from_code(code.arg(0)?)
        .ok_or_else(|| bad(format!("line {}: vertex declaration {} is not known", code.line, code.args().join(" "))))
}

/// The indices (read at `line`) must be whole triangles of the `n` vertices.
fn check_indices(indices: &[u32], n: usize, line: usize) -> Result<(), ResourceError> {
    if let Some(&bad_index) = indices.iter().find(|&&i| i as usize >= n) {
        return Err(bad(format!("line {line}: index {bad_index} with {n} vertices")));
    }
    if !indices.len().is_multiple_of(3) {
        return Err(bad(format!("line {line}: {} indices are not whole triangles", indices.len())));
    }
    Ok(())
}

/// Per vertex component: its index and the value of every vertex.
type Attributes = Vec<(usize, Vec<[f32; 4]>)>;

fn geometry_indices(node: &Node) -> Result<Vec<u32>, ResourceError> {
    let mut indices = Vec::with_capacity(node.int::<usize>().unwrap_or(0));
    for row in node.children() {
        for w in &row.words {
            indices.push(w.parse().map_err(|_| bad(format!("line {}: {w} is not an index", row.line)))?);
        }
    }
    Ok(indices)
}

/// The vertex rows as one attribute list per component of the declaration.
fn geometry_attributes(vertices: &Node, decl: &VertexDecl) -> Result<Attributes, ResourceError> {
    let components: Vec<(usize, u8)> = decl.components().collect();
    let mut attributes: Attributes =
        components.iter().map(|&(k, _)| (k, Vec::with_capacity(vertices.children().len()))).collect();
    for v in vertices.children() {
        let groups: Vec<&[String]> = v.words.split(|w| w == "/").collect();
        if groups.len() != components.len() {
            return Err(bad(format!(
                "line {}: {} vertex components, the declaration has {}",
                v.line,
                groups.len(),
                components.len()
            )));
        }
        for (i, (&(k, ty), words)) in components.iter().zip(groups).enumerate() {
            attributes[i].1.push(component_value(v, i, VertexDecl::width(k, ty), words)?);
        }
    }
    Ok(attributes)
}

fn component_value(v: &Node, i: usize, width: usize, words: &[String]) -> Result<[f32; 4], ResourceError> {
    if words.len() != width {
        return Err(bad(format!("line {}: component {} has {} numbers, not {width}", v.line, i + 1, words.len())));
    }
    let mut value = [0.0f32; 4];
    for (x, w) in value.iter_mut().zip(words) {
        *x = text::parse_f32(w, v.line)?;
    }
    Ok(value)
}
