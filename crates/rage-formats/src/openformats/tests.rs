use std::collections::HashMap;

use rage_resource::{Resource, ResourceError};

use super::*;
use crate::texture::{Dimension, Format};

fn files_source(files: &[ExportedFile]) -> impl Fn(&str) -> Result<Vec<u8>, ResourceError> + '_ {
    let map: HashMap<&str, &[u8]> = files.iter().map(|f| (f.path.as_str(), f.data.as_slice())).collect();
    move |p: &str| map.get(p).map(|d| d.to_vec()).ok_or_else(|| ResourceError::Format(format!("no file {p}")))
}

fn text_of<'a>(files: &'a [ExportedFile], path: &str) -> &'a str {
    let f = files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path} not written"));
    std::str::from_utf8(&f.data).unwrap()
}

#[test]
fn declaration_codes() {
    // Codes seen in OpenIV's exports.
    let d = VertexDecl { flags: 0x4059, types: DEFAULT_TYPES };
    assert_eq!(d.code(), "N51263BB5");
    assert_eq!(VertexDecl::from_code("N51263BB5"), Some(d));
    assert_eq!(VertexDecl { flags: 0x405F, types: DEFAULT_TYPES }.code(), "S9445853F");
    // Unused type bits do not change the code.
    assert_eq!(VertexDecl { flags: 0x4059, types: DEFAULT_TYPES ^ 0xF00 }.code(), "N51263BB5");
    // Others carry the layout itself.
    let odd = VertexDecl { flags: 0x00D9, types: 0x7755_5555_5599_6996 };
    let code = odd.code();
    assert_eq!(code, "N00D97755555555996996");
    assert_eq!(VertexDecl::from_code(&code), Some(odd));
    let half = VertexDecl { flags: 0x0041, types: 0x1006 };
    assert!(half.code().starts_with('N'));
    assert_eq!(VertexDecl::from_code(&half.code()), Some(half));
    assert_eq!(VertexDecl::from_code("Nnonsense"), None);
}

#[test]
fn numbers_like_openiv() {
    assert_eq!(text::num(0.0), "0.00000000");
    assert_eq!(text::num(-0.0), "0.00000000");
    assert_eq!(text::num(9998.0), "9998.00000000");
    assert_eq!(text::num(58.368), "58.36800000");
    assert_eq!(text::num(0.97), "0.97000000");
    assert_eq!(text::num(-0.235_351_56), "-0.23535160");
}

#[test]
fn blocks_and_errors() {
    let doc = text::parse("Version 165 32\r\n{\r\n\tA 1 2\r\n\tB\r\n\t{\r\n\t\tC x\r\n\t}\r\n}\r\n").unwrap();
    assert_eq!(doc.root.get("a").unwrap().floats().unwrap(), vec![1.0, 2.0]);
    assert_eq!(doc.root.get("B").unwrap().get("C").unwrap().arg(0).unwrap(), "x");
    assert!(text::parse("Version 1 2\n{\nA\n").is_err());
    assert!(text::parse("Version 1 2\n{\n}\n}\n").is_err());
    assert!(text::parse("A 1\n{\n}\n").is_err());
    assert!(text::parse("{\n}\n").is_err());
}

/// A drawable with everything the format has: two detail levels, a fixed and
/// a skinned model, a skeleton whose bones are not in tree order, shader
/// parameters and an embedded texture.
fn sample() -> OfDrawable {
    let tri = |shader: u16, decl: VertexDecl, n: usize| {
        let mut attributes = Vec::new();
        for (k, _) in decl.components() {
            let values = (0..n)
                .map(|i| {
                    let i = i as f32;
                    match k {
                        component::BLEND_INDICES => [0.0, 0.0, (i as u32 % 3) as f32, 1.0],
                        component::BLEND_WEIGHTS => [0.0, 0.0, 1.0, 0.0],
                        component::COLOUR0 | component::COLOUR1 => [255.0, 128.0, 0.0, 255.0],
                        _ => [i * 0.5 - 1.25, 1.0 / (i + 3.0), -i * 1234.567, 1.0],
                    }
                })
                .collect();
            attributes.push((k, values));
        }
        OfGeometry { shader, decl, indices: (0..n as u32).collect(), attributes }
    };
    let fixed = VertexDecl { flags: 0x4059, types: DEFAULT_TYPES };
    let skinned = VertexDecl { flags: 0x405F, types: DEFAULT_TYPES };
    let model = |skinned_model: bool, geometries: Vec<OfGeometry>| OfModel {
        bone: if skinned_model { 0 } else { 2 },
        skinned: skinned_model,
        bone_count: if skinned_model { 3 } else { 0 },
        mask: 255,
        bounds: vec![([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0]); geometries.len() + usize::from(geometries.len() > 1)],
        geometries,
    };
    let bone = |name: &str, tag: u16, parent: i16, flags: u16| OfBone {
        name: name.into(),
        tag,
        mirror: tag,
        parent,
        flags,
        rotation: [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2],
        translation: [0.25, -1.5, 3.0],
        scale: [1.0; 3],
    };
    let texture = crate::texture::Texture {
        name: "Prop_Thing_D".into(),
        width: 4,
        height: 4,
        depth: 1,
        levels: 1,
        format: Format::BC1_UNORM,
        dimension: Dimension::Texture2D,
        usage: 20,
        data: (0..8).collect(),
    };
    let mut d = OfDrawable {
        name: "prop_thing".into(),
        shaders: vec![
            OfShader {
                name: "normal_spec".into(),
                params: vec![
                    ("DiffuseSampler".into(), OfValue::Texture("Prop_Thing_D".into())),
                    ("BumpSampler".into(), OfValue::Texture("prop_thing_n".into())),
                    ("Bumpiness".into(), OfValue::Floats(vec![1.5])),
                    ("SpecMapIntMask".into(), OfValue::Floats(vec![1.0, 0.0, 0.0])),
                    ("hash_12345678".into(), OfValue::Floats(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0])),
                ],
            },
            OfShader { name: "hash_ABCDEF01".into(), params: Vec::new() },
        ],
        skeleton: Some(OfSkeleton {
            crc: 3_624_623_659,
            // Bone 1's child comes after bone 2, a child of the root.
            bones: vec![
                bone("root", 0, -1, 0x1077),
                bone("arm", 100, 0, 0x7),
                bone("leg", 200, 0, 0x77),
                bone("hand", 300, 1, 0x7),
            ],
        }),
        center: [0.5, 0.25, 1.0],
        radius: 2.5,
        aabb_min: [-1.0, -2.0, -3.0],
        aabb_max: [1.0, 2.0, 3.0],
        textures: vec![OfTexture { texture }],
        ..Default::default()
    };
    d.lods[0] = OfLod {
        distance: 50.0,
        models: vec![model(false, vec![tri(0, fixed, 6), tri(1, fixed, 3)]), model(true, vec![tri(0, skinned, 9)])],
    };
    d.lods[1] = OfLod { distance: 200.0, models: vec![model(false, vec![tri(1, fixed, 3)])] };
    d
}

#[test]
fn drawable_round_trip() {
    let d = sample();
    let files = export_odr(&d);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    for p in [
        "prop_thing.odr",
        "prop_thing/prop_thing.skel",
        "prop_thing/prop_thing_high_0.mesh",
        "prop_thing/prop_thing_high_1.mesh",
        "prop_thing/prop_thing_med.mesh",
        "prop_thing/prop_thing.otd",
        "prop_thing/prop_thing_d.otx",
        "prop_thing/prop_thing_d.dds",
    ] {
        assert!(paths.contains(&p), "{p} missing from {paths:?}");
    }
    let odr = text_of(&files, "prop_thing.odr");
    assert!(odr.starts_with("Version 165 32\r\n{\r\n\tShaders\r\n"));
    assert!(odr.contains("\t\tnormal_spec.sps\r\n"));
    assert!(odr.contains("\t\t\tBumpiness 1.50000000\r\n"));
    assert!(odr.contains("\tSkeleton prop_thing\\prop_thing.skel\r\n"));
    assert!(odr.contains("\tTextureDictionary prop_thing\\prop_thing.otd\r\n"));
    // The fixed model sits on "leg", the third bone in the tree.
    assert!(odr.contains("\t\t\tprop_thing\\prop_thing_high_0.mesh 3\r\n"), "{odr}");
    assert!(odr.contains("\t\tMed 200.00000000\r\n"));
    assert!(odr.contains("\t\tLow 9998.00000000\r\n\t\tVlow"));
    let skel = text_of(&files, "prop_thing/prop_thing.skel");
    assert!(skel.contains("\tNumBones 4\r\n\tBone root 0\r\n"));
    assert!(skel.contains("\t\tFlags ROT_X ROT_Y ROT_Z TRANS_X TRANS_Y TRANS_Z\r\n"));
    assert!(skel.contains("\t\tChildren 2\r\n"));
    let mesh = text_of(&files, "prop_thing/prop_thing_high_1.mesh");
    assert!(mesh.contains("\tSkinned True\r\n\tBoneCount 3\r\n\tMask 255\r\n"));
    assert!(mesh.contains("VertexDeclaration S9445853F"));
    // Blend index 2 ("leg") is the fourth bone in the tree.
    assert!(mesh.contains(" / 0 0 3 1 / "), "{mesh}");
    assert!(mesh.contains("\t\t\t\t0 1 2 3 4 5 6 7 8\r\n"));
    let otx = text_of(&files, "prop_thing/prop_thing_d.otx");
    assert!(otx.contains(
        "\tImage prop_thing_d.dds\r\n\tType Regular\r\n\tPixelFormat DXT1\r\n\tLevels 1\r\n\tUsage DIFFUSE\r\n"
    ));

    let src = files_source(&files);
    let back = parse_odr(&src, "prop_thing.odr").unwrap();
    assert_eq!(difference(&d, &back), None);
    assert_eq!(
        back.skeleton.as_ref().unwrap().bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["root", "arm", "hand", "leg"]
    );
}

#[test]
fn differences_are_found() {
    let d = sample();
    let files = export_odr(&d);
    let src = files_source(&files);
    let back = parse_odr(&src, "prop_thing.odr").unwrap();
    let mut moved = back.clone();
    moved.lods[0].models[0].geometries[0].attributes[0].1[2][1] += 0.01;
    assert!(difference(&d, &moved).unwrap().contains("vertex component 0"));
    let mut reshaded = back.clone();
    reshaded.shaders[0].params[2].1 = OfValue::Floats(vec![2.0]);
    assert!(difference(&d, &reshaded).unwrap().contains("Bumpiness"));
    let mut rebound = back.clone();
    rebound.lods[0].models[1].geometries[0].attributes[2].1[0][2] = 1.0;
    assert!(difference(&d, &rebound).is_some());
    let mut retextured = back;
    retextured.textures[0].texture.data[0] ^= 1;
    assert!(difference(&d, &retextured).is_some());
}

#[test]
fn files_with_errors() {
    let files = export_odr(&sample());
    let edit = |path: &str, f: &dyn Fn(&str) -> String| -> Vec<ExportedFile> {
        files
            .iter()
            .map(|x| {
                let data =
                    if x.path == path { f(std::str::from_utf8(&x.data).unwrap()).into_bytes() } else { x.data.clone() };
                ExportedFile { path: x.path.clone(), data }
            })
            .collect()
    };
    let parse =
        |files: &[ExportedFile]| parse_odr(&files_source(files), "prop_thing.odr").map(|_| ()).unwrap_err().to_string();
    let e = parse(&edit("prop_thing/prop_thing_med.mesh", &|t| t.replacen(" / ", " ", 1)));
    assert!(e.contains("prop_thing_med.mesh") && e.contains("vertex components"), "{e}");
    let e = parse(&edit("prop_thing/prop_thing_med.mesh", &|t| t.replace("\t\t\t\t0 1 2\r\n", "\t\t\t\t0 1 7\r\n")));
    assert!(e.contains("index 7"), "{e}");
    let e = parse(&edit("prop_thing/prop_thing_med.mesh", &|t| t.replace("N51263BB5", "N0BADC0DE")));
    assert!(e.contains("not known"), "{e}");
    let e = parse(&edit("prop_thing.odr", &|t| t.replace("prop_thing_med.mesh", "missing.mesh")));
    assert!(e.contains("missing.mesh"), "{e}");
}

#[test]
fn paths_with_backslashes() {
    assert_eq!(parse::resolve("a/b", "c\\d.mesh"), "a/b/c/d.mesh");
    assert_eq!(parse::resolve("a/b", "..\\c.otx"), "a/c.otx");
    assert_eq!(parse::resolve("", "x/./y.dds"), "x/y.dds");
}

#[test]
fn dictionaries_and_fragments() {
    let mut a = sample();
    a.name = "head_000_r".into();
    let mut b = sample();
    b.name = "hash_00C0FFEE".into();
    b.skeleton = None;
    for m in b.lods.iter_mut().flat_map(|l| &mut l.models) {
        m.bone = 0;
    }
    let files = export_odd("ped_parts", &[a.clone(), b.clone()]);
    let odd = text_of(&files, "ped_parts.odd");
    assert!(odd.contains("\tped_parts\\head_000_r.odr\r\n\tped_parts\\hash_00c0ffee.odr\r\n"), "{odd}");
    let back = parse_odd(&files_source(&files), "ped_parts.odd").unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(difference(&a, &back[0]), None);
    assert_eq!(difference(&b, &back[1]), None);

    let f = OfFragment { drawable: a.clone(), children: vec![(1234, b.clone()), (5678, b.clone())] };
    let files = export_oft(&f);
    let oft = text_of(&files, "head_000_r.oft");
    assert!(oft.starts_with("Version 162 30\r\n{\r\n\tDrawable\r\n\t{\r\n\t\tShaders\r\n"), "{oft}");
    assert!(oft.contains("\t\t\tBoneTag 5678\r\n"));
    let back = parse_oft(&files_source(&files), "head_000_r.oft").unwrap();
    assert_eq!(difference(&f.drawable, &back.drawable), None);
    assert_eq!(back.children.iter().map(|c| c.0).collect::<Vec<_>>(), [1234, 5678]);
    for (_, c) in &back.children {
        assert_eq!(difference(&b, c), None);
    }

    let textures = sample().textures;
    let files = export_otd("prop_textures", &textures);
    assert!(text_of(&files, "prop_textures.otd").contains("\tprop_textures\\prop_thing_d.otx\r\n"));
    let back = parse_otd(&files_source(&files), "prop_textures.otd").unwrap();
    assert_eq!(back.len(), 1);
    // Names come from the file names (lowercase); the game finds textures by name in any case.
    assert_eq!(texture_difference(&textures[0].texture, &back[0].texture), None);
}

/// A real drawable from the model writer's test template: read, written as
/// text, read back.
#[test]
fn from_a_resource() {
    let res: Resource = crate::ydr_write::tests::template_with(2, true);
    let d = read_ydr(&res, "template").unwrap();
    assert_eq!(d.shaders.len(), 2);
    assert_eq!(d.textures.len(), 1);
    assert_eq!(d.lods[0].models[0].geometries.len(), 2);
    let g = &d.lods[0].models[0].geometries[0];
    assert_eq!(g.decl.flags, 0x4059, "position, normal, colour, uv, tangent");
    assert_eq!(g.attribute(component::POSITION).unwrap()[1], [1.0, 0.0, 0.0, 0.0]);
    let files = export_odr(&d);
    let back = parse_odr(&files_source(&files), "template.odr").unwrap();
    assert_eq!(difference(&d, &back), None);
}

#[test]
fn enhanced_parameter_names() {
    assert_eq!(read::legacy_texture_param("DiffuseTex"), "DiffuseSampler");
    assert_eq!(read::legacy_texture_param("DiffuseTex2"), "DiffuseSampler2");
    assert_eq!(read::legacy_texture_param("SpecularTex"), "SpecSampler");
    assert_eq!(read::legacy_texture_param("DamageTexture"), "DamageSampler");
    assert_eq!(read::legacy_texture_param("hash_12345678"), "hash_12345678");
}

#[test]
fn texture_names_from_file_names() {
    assert_eq!(file_stem("generic_ cav_detail_dash"), "generic__cav_detail_dash");
    assert!(same_texture_name("generic_ cav_detail_dash", "generic__cav_detail_dash"));
    assert!(same_texture_name("Prop_Thing_D", "prop_thing_d"));
    assert!(!same_texture_name("prop_thing_d", "prop_thing_n"));
}
