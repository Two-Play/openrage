//! Writing the openFormats model as text files.

use super::text::{Writer, num, nums};
use super::*;
use crate::texture::{Dimension, Format};

/// A file to write, at a path relative to the export folder (`/`-separated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    pub path: String,
    pub data: Vec<u8>,
}

fn version(w: &mut Writer, (major, minor): (u32, u32)) {
    w.line(format!("Version {major} {minor}"));
}

/// Joins folder parts with `/` (files) or `\` (paths inside the text).
fn join(sep: char, parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(&sep.to_string())
}

/// Whether a texture name read back from a file name (`.otx`) is the texture
/// `name`: file names are lowercase and have `_` for characters such as spaces.
pub fn same_texture_name(name: &str, from_file: &str) -> bool {
    name.eq_ignore_ascii_case(from_file) || file_stem(name) == file_stem(from_file)
}

/// A file name for a texture or drawable name.
pub fn file_stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "_-.+".contains(c) { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if s.is_empty() { "unnamed".to_owned() } else { s }
}

/// A drawable (`.ydr`): `<name>.odr` and the folder `<name>/` with its
/// meshes, skeleton and embedded textures.
pub fn export_odr(d: &OfDrawable) -> Vec<ExportedFile> {
    let mut files = Vec::new();
    odr_file(d, "", &mut files);
    files
}

fn odr_file(d: &OfDrawable, base: &str, files: &mut Vec<ExportedFile>) -> String {
    let stem = file_stem(&d.name);
    let mut w = Writer::default();
    version(&mut w, VERSION_DRAWABLE);
    w.open();
    drawable_body(&mut w, d, &stem, base, &stem, files);
    w.close();
    let path = join('/', &[base, &format!("{stem}.odr")]);
    files.push(ExportedFile { path: path.clone(), data: w.out.into_bytes() });
    path
}

/// The contents of a drawable block. Its files go into the folder `dir`
/// (relative to the text file, which is in `base`); they are named after `stem`.
fn drawable_body(w: &mut Writer, d: &OfDrawable, dir: &str, base: &str, stem: &str, files: &mut Vec<ExportedFile>) {
    let folder = join('/', &[base, dir]);
    let rank = d.skeleton.as_ref().map(bone_ranks).unwrap_or_default();
    let remap = |b: usize| rank.get(b).copied().unwrap_or(b);
    w.block("Shaders", |w| {
        for s in &d.shaders {
            w.block(format!("{}.sps", s.name), |w| {
                for (name, value) in &s.params {
                    match value {
                        OfValue::Texture(t) => w.line(format!("{name} {}", if t.is_empty() { "null" } else { t })),
                        OfValue::Floats(v) => w.line(format!("{name} {}", nums(v))),
                    }
                }
            });
        }
    });
    if !d.textures.is_empty() {
        let otd = format!("{stem}.otd");
        files.extend(textures_files(&d.textures, &folder, &otd, ""));
        w.line(format!("TextureDictionary {}", join('\\', &[dir, &otd])));
    }
    match &d.skeleton {
        Some(s) => {
            let skel = format!("{stem}.skel");
            files.push(ExportedFile { path: join('/', &[&folder, &skel]), data: skeleton_text(s).into_bytes() });
            w.line(format!("Skeleton {}", join('\\', &[dir, &skel])));
        }
        None => w.line("Skeleton null"),
    }
    w.block("LodGroup", |w| {
        w.line(format!("Center {}", nums(&d.center)));
        w.line(format!("Radius {}", num(d.radius)));
        w.line(format!("AABBMin {}", nums(&d.aabb_min)));
        w.line(format!("AABBMax {}", nums(&d.aabb_max)));
        for (k, lod) in d.lods.iter().enumerate() {
            w.line(format!("{} {}", LOD_NAMES[k], num(lod.distance)));
            if lod.models.is_empty() {
                continue;
            }
            w.open();
            for (i, m) in lod.models.iter().enumerate() {
                let level = LOD_NAMES[k].to_ascii_lowercase();
                let mesh = if lod.models.len() == 1 {
                    format!("{stem}_{level}.mesh")
                } else {
                    format!("{stem}_{level}_{i}.mesh")
                };
                files
                    .push(ExportedFile { path: join('/', &[&folder, &mesh]), data: mesh_text(m, &remap).into_bytes() });
                w.line(format!("{} {}", join('\\', &[dir, &mesh]), remap(m.bone as usize)));
            }
            w.close();
        }
    });
    w.line("Joints null");
    w.line("Light null");
    w.line("Bound null");
}

/// Children of every bone, in index order, and the roots.
fn bone_tree(s: &OfSkeleton) -> (Vec<Vec<usize>>, Vec<usize>) {
    let n = s.bones.len();
    let mut children = vec![Vec::new(); n];
    let mut roots = Vec::new();
    for i in 0..n {
        match usize::try_from(s.bones[i].parent).ok().filter(|&p| p < i) {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    (children, roots)
}

/// Where each bone ends up in the `.skel` file, which lists the bones as a
/// tree (depth first): usually where it already is.
pub(crate) fn bone_ranks(s: &OfSkeleton) -> Vec<usize> {
    let (children, roots) = bone_tree(s);
    let mut rank = vec![0; s.bones.len()];
    let mut next = 0;
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(b) = stack.pop() {
        rank[b] = next;
        next += 1;
        stack.extend(children[b].iter().rev());
    }
    rank
}

fn skeleton_text(s: &OfSkeleton) -> String {
    let mut w = Writer::default();
    version(&mut w, VERSION_SKELETON);
    w.open();
    w.line(format!("DataCRC {}", s.crc));
    w.line(format!("NumBones {}", s.bones.len()));
    fn bone(w: &mut Writer, s: &OfSkeleton, i: usize, children: &[Vec<usize>]) {
        let b = &s.bones[i];
        w.block(format!("Bone {} {}", if b.name.is_empty() { "unnamed" } else { &b.name }, b.tag), |w| {
            w.line(format!("MirrorBoneId {}", b.mirror));
            let flags: Vec<&str> = BONE_FLAGS.iter().filter(|(bit, _)| b.flags & bit != 0).map(|(_, n)| *n).collect();
            w.line(format!("Flags {}", if flags.is_empty() { "-".to_owned() } else { flags.join(" ") }));
            w.line(format!("RotationQuaternion {}", nums(&b.rotation)));
            w.line(format!("LocalOffset {}", nums(&b.translation)));
            w.line(format!("Scale {}", nums(&b.scale)));
            if !children[i].is_empty() {
                w.block(format!("Children {}", children[i].len()), |w| {
                    for &c in &children[i] {
                        bone(w, s, c, children);
                    }
                });
            }
        });
    }
    let (children, roots) = bone_tree(s);
    for r in roots {
        bone(&mut w, s, r, &children);
    }
    w.close();
    w.out
}

fn mesh_text(m: &OfModel, remap: &dyn Fn(usize) -> usize) -> String {
    let mut w = Writer::default();
    version(&mut w, VERSION_DRAWABLE);
    w.open();
    w.line("Locked False");
    w.line(format!("Skinned {}", if m.skinned { "True" } else { "False" }));
    w.line(format!("BoneCount {}", m.bone_count));
    w.line(format!("Mask {}", m.mask));
    w.block("Bounds", |w| {
        for (min, max) in &m.bounds {
            w.block("Aabb", |w| {
                w.line(format!("Min {}", nums(min)));
                w.line(format!("Max {}", nums(max)));
            });
        }
    });
    w.block("Geometries", |w| {
        for g in &m.geometries {
            w.block("Geometry", |w| {
                w.line(format!("ShaderIndex {}", g.shader));
                w.line("Flags -");
                w.line(format!("VertexDeclaration {}", g.decl.code()));
                w.block(format!("Indices {}", g.indices.len()), |w| {
                    for row in g.indices.chunks(15) {
                        w.line(row.iter().map(u32::to_string).collect::<Vec<_>>().join(" "));
                    }
                });
                let types: Vec<(usize, u8)> = g.decl.components().collect();
                w.block(format!("Vertices {}", g.vertex_count()), |w| {
                    let mut line = String::new();
                    for i in 0..g.vertex_count() {
                        line.clear();
                        for (n, (k, values)) in g.attributes.iter().enumerate() {
                            if n > 0 {
                                line.push_str(" / ");
                            }
                            let ty = types.iter().find(|(c, _)| c == k).map_or(7, |t| t.1);
                            let width = VertexDecl::width(*k, ty);
                            let mut v = values[i];
                            if *k == component::BLEND_INDICES {
                                v = v.map(|b| remap(b as usize) as f32);
                            }
                            let v = &v[..width];
                            if VertexDecl::integral(*k) {
                                let ints: Vec<String> = v.iter().map(|x| (x.round() as i64).to_string()).collect();
                                line.push_str(&ints.join(" "));
                            } else {
                                line.push_str(&nums(v));
                            }
                        }
                        w.line(&line);
                    }
                });
            });
        }
    });
    w.close();
    w.out
}

/// A fragment (`.yft`): `<name>.oft` with the main drawable and the parts,
/// their files in the folder `<name>/`.
pub fn export_oft(f: &OfFragment) -> Vec<ExportedFile> {
    let stem = file_stem(&f.drawable.name);
    let mut files = Vec::new();
    let mut w = Writer::default();
    version(&mut w, VERSION_FRAGMENT);
    w.open();
    w.block("Drawable", |w| drawable_body(w, &f.drawable, &stem, "", &stem, &mut files));
    if !f.children.is_empty() {
        w.block("Children", |w| {
            for (tag, d) in &f.children {
                w.block("Child", |w| {
                    w.line(format!("BoneTag {tag}"));
                    w.block("Drawable", |w| drawable_body(w, d, &stem, "", &file_stem(&d.name), &mut files));
                });
            }
        });
    }
    w.close();
    files.push(ExportedFile { path: format!("{stem}.oft"), data: w.out.into_bytes() });
    files
}

/// A drawable dictionary (`.ydd`): `<name>.odd` and one `.odr` per drawable
/// in the folder `<name>/`.
pub fn export_odd(name: &str, drawables: &[OfDrawable]) -> Vec<ExportedFile> {
    let stem = file_stem(name);
    let mut files = Vec::new();
    let mut w = Writer::default();
    version(&mut w, VERSION_DRAWABLE);
    w.open();
    for d in drawables {
        let path = odr_file(d, &stem, &mut files);
        w.line(path.replace('/', "\\"));
    }
    w.close();
    files.push(ExportedFile { path: format!("{stem}.odd"), data: w.out.into_bytes() });
    files
}

/// A texture dictionary (`.ytd`): `<name>.otd` and the folder `<name>/` with
/// one `.otx` and `.dds` per texture.
pub fn export_otd(name: &str, textures: &[OfTexture]) -> Vec<ExportedFile> {
    let stem = file_stem(name);
    textures_files(textures, "", &format!("{stem}.otd"), &stem)
}

/// The `.otd` at `base/otd` listing textures in `base/dir`.
fn textures_files(textures: &[OfTexture], base: &str, otd: &str, dir: &str) -> Vec<ExportedFile> {
    let mut files = Vec::new();
    let mut w = Writer::default();
    version(&mut w, VERSION_TEXTURES);
    w.open();
    for t in textures {
        let t = &t.texture;
        let stem = file_stem(&t.name);
        let folder = join('/', &[base, dir]);
        files.push(ExportedFile { path: join('/', &[&folder, &format!("{stem}.dds")]), data: crate::dds::to_dds(t) });
        let mut x = Writer::default();
        version(&mut x, VERSION_TEXTURES);
        x.open();
        x.line(format!("Image {stem}.dds"));
        x.line(format!(
            "Type {}",
            match t.dimension {
                Dimension::Texture2D => "Regular",
                Dimension::Cube => "Cube",
                Dimension::Texture3D => "Volume",
            }
        ));
        x.line(format!("PixelFormat {}", pixel_format(t.format)));
        x.line(format!("Levels {}", t.levels));
        x.line(format!("Usage {}", USAGES.iter().find(|(u, _)| *u == t.usage).map_or("DEFAULT", |u| u.1)));
        x.line("UsageFlags -");
        x.line("ExtraFlags 0");
        x.close();
        files.push(ExportedFile { path: join('/', &[&folder, &format!("{stem}.otx")]), data: x.out.into_bytes() });
        w.line(join('\\', &[dir, &format!("{stem}.otx")]));
    }
    w.close();
    files.push(ExportedFile { path: join('/', &[base, otd]), data: w.out.into_bytes() });
    files
}

/// Pixel format names as OpenIV writes them (Direct3D 9 style where there is one).
fn pixel_format(f: Format) -> String {
    match f {
        Format::BC1_UNORM => "DXT1".into(),
        Format::BC2_UNORM => "DXT3".into(),
        Format::BC3_UNORM => "DXT5".into(),
        Format::BC4_UNORM => "ATI1".into(),
        Format::BC5_UNORM => "ATI2".into(),
        Format::B8G8R8A8_UNORM => "A8R8G8B8".into(),
        Format::B8G8R8X8_UNORM => "X8R8G8B8".into(),
        Format::R8G8B8A8_UNORM => "A8B8G8R8".into(),
        Format::B5G5R5A1_UNORM => "A1R5G5B5".into(),
        Format::B5G6R5_UNORM => "R5G6B5".into(),
        Format::A8_UNORM => "A8".into(),
        Format::R8_UNORM => "L8".into(),
        other => other.name(),
    }
}
