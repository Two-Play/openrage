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
    assert!(json.contains("\"name\":\"root\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],\"children\":[4]"), "{json}");
    assert!(json.contains("\"skins\":[{\"joints\":[3,4],\"inverseBindMatrices\":6}]"), "{json}");
    let bin = &glb[28 + json_len..];
    assert_eq!(&bin[84..92], &[0, 0, 1, 0, 0, 0, 0, 0]);
    let get_f32 = |offset| f32::from_le_bytes(bin[offset..offset + 4].try_into().unwrap());
    assert_eq!((get_f32(108), get_f32(112), get_f32(116)), (0.25, 0.75, 0.0));
    assert_eq!(get_f32(280), -1.0);
}

use crate::drawable::{Bone, param};
use crate::texture::{Dimension, Format};

/// The JSON and binary chunks of a `.glb`, after checking its container.
fn split(glb: &[u8]) -> (String, Vec<u8>) {
    assert_eq!(&glb[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    assert_eq!(json_len % 4, 0);
    let json = String::from_utf8(glb[20..20 + json_len].to_vec()).unwrap();
    let bin_at = 20 + json_len;
    let bin_len = u32::from_le_bytes(glb[bin_at..bin_at + 4].try_into().unwrap()) as usize;
    assert_eq!(&glb[bin_at + 4..bin_at + 8], b"BIN\0");
    assert_eq!(bin_len % 4, 0);
    assert_eq!(bin_at + 8 + bin_len, glb.len());
    (json, glb[bin_at + 8..].to_vec())
}

fn triangle_mesh(shader: u16) -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        indices: vec![0, 1, 2],
        shader,
        ..Default::default()
    }
}

fn shader(textures: &[(u32, &str)]) -> Shader {
    Shader {
        name_hash: 1,
        textures: textures.iter().map(|(_, n)| n.to_string()).collect(),
        samplers: textures.iter().map(|(h, n)| (*h, n.to_string())).collect(),
    }
}

fn rgba_texture(name: &str) -> Texture {
    Texture {
        name: name.into(),
        width: 2,
        height: 2,
        depth: 1,
        levels: 1,
        format: Format::R8G8B8A8_UNORM,
        dimension: Dimension::Texture2D,
        usage: 0,
        data: (0..16).collect(),
    }
}

fn single(meshes: Vec<Mesh>, shaders: Vec<Shader>) -> Drawable {
    Drawable { meshes, shaders, ..Default::default() }
}

fn identity_node(name: &str, mesh: usize) -> Node {
    Node { name: name.into(), mesh, transform: math::IDENTITY }
}

#[test]
fn names_are_escaped_for_json() {
    assert_eq!(esc("plain"), "plain");
    assert_eq!(esc("a\"b\\c"), "a\\\"b\\\\c");
    assert_eq!(esc("tab\there\n"), "tab\\u0009here\\u000a");
    assert_eq!(esc("ünï"), "ünï");
}

#[test]
fn numbers_that_are_not_finite_become_zero() {
    assert_eq!(finite(1.5), 1.5);
    assert_eq!(finite(f32::NAN), 0.0);
    assert_eq!(finite(f32::INFINITY), 0.0);
    assert_eq!(finite(f32::NEG_INFINITY), 0.0);
    assert_eq!(matrix_json(&[f32::NAN; 16]), vec!["0"; 16].join(","));
}

#[test]
fn normals_are_made_unit_length() {
    let n = unit_normals(&[[0.0, 0.0, 2.0], [3.0, 0.0, 4.0], [0.0; 3], [1e-9, 0.0, 0.0]]);
    assert_eq!(n, [[0.0, 0.0, 1.0], [0.6, 0.0, 0.8], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
}

#[test]
fn skin_weights_need_one_usable_influence_per_vertex() {
    let skinned = |joints: [u16; 4], weights: [f32; 4]| Mesh {
        bone_indices: vec![joints; 3],
        bone_weights: vec![weights; 3],
        ..triangle_mesh(0)
    };
    assert!(has_skin_weights(&skinned([0, 1, 0, 0], [0.5, 0.5, 0.0, 0.0]), 2));
    assert!(!has_skin_weights(&skinned([0, 1, 0, 0], [0.0; 4]), 2), "weights that add up to nothing");
    assert!(!has_skin_weights(&skinned([0, 5, 0, 0], [0.5, 0.5, 0.0, 0.0]), 2), "a bone the skeleton lacks");
    assert!(has_skin_weights(&skinned([0, 5, 0, 0], [1.0, 0.0, 0.0, 0.0]), 2), "an unused influence is not checked");
    assert!(!has_skin_weights(&skinned([0, 1, 0, 0], [1.5, -0.5, 0.0, 0.0]), 2), "a negative weight");
    assert!(!has_skin_weights(&skinned([0, 1, 0, 0], [f32::NAN, 1.0, 0.0, 0.0]), 2));
    assert!(!has_skin_weights(&skinned([0, 1, 0, 0], [f32::INFINITY, 0.0, 0.0, 0.0]), 2));
}

#[test]
fn skin_weights_need_a_mesh_that_is_fully_resolved() {
    let ok = Mesh { bone_indices: vec![[0; 4]; 3], bone_weights: vec![[1.0, 0.0, 0.0, 0.0]; 3], ..triangle_mesh(0) };
    assert!(has_skin_weights(&ok, 1));
    assert!(!has_skin_weights(&Mesh::default(), 1));
    assert!(!has_skin_weights(&Mesh { bone_tags: vec![1], ..ok.clone() }, 1));
    assert!(!has_skin_weights(&Mesh { bone_hashes: vec![1], ..ok.clone() }, 1));
    assert!(!has_skin_weights(&Mesh { bone_indices: vec![[0; 4]; 2], ..ok.clone() }, 1));
    assert!(!has_skin_weights(&Mesh { bone_weights: Vec::new(), ..ok }, 1));
}

#[test]
fn influences_on_the_same_joint_are_merged_and_weights_normalized() {
    let mesh = Mesh {
        bone_indices: vec![[1, 1, 2, 2], [3, 4, 5, 6]],
        bone_weights: vec![[0.25, 0.25, 0.25, 0.25], [2.0, 2.0, 0.0, 0.0]],
        ..triangle_mesh(0)
    };
    let (joints, weights) = normalized_skin(&mesh);
    assert_eq!(joints, [[1, 0, 2, 0], [3, 4, 0, 0]]);
    assert_eq!(weights, [[0.5, 0.0, 0.5, 0.0], [0.5, 0.5, 0.0, 0.0]]);
}

fn bone(name: &str, parent: i16, translation: [f32; 3]) -> Bone {
    Bone {
        name: name.into(),
        parent,
        translation,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
        ..Default::default()
    }
}

#[test]
fn joint_nodes_list_children_and_roots() {
    let skeleton = Skeleton {
        bones: vec![
            bone("root", -1, [0.0; 3]),
            bone("a\"b", 0, [1.0, 2.0, 3.0]),
            bone("c", 1, [0.0; 3]),
            bone("d", 7, [0.0; 3]),
        ],
    };
    let (roots, nodes) = joint_nodes(&skeleton, 10);
    assert_eq!(roots, [10, 13], "a parent that comes later makes a root");
    assert_eq!(nodes[0], "{\"name\":\"root\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],\"children\":[11]}");
    assert_eq!(nodes[1], "{\"name\":\"a\\\"b\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,1,2,3,1],\"children\":[12]}");
    assert!(!nodes[2].contains("children"));
}

#[test]
fn a_drawable_without_meshes_is_an_empty_scene() {
    let (json, bin) = split(&write_glb(&Drawable::default(), "empty", &|_| None));
    assert!(json.contains("\"meshes\":[]"), "{json}");
    assert!(json.contains("\"nodes\":[{\"name\":\"empty\""), "{json}");
    assert!(json.contains("\"children\":[]"), "{json}");
    assert!(bin.is_empty());
}

#[test]
fn attributes_are_written_when_a_mesh_has_them() {
    let mut mesh = triangle_mesh(0);
    mesh.normals = vec![[0.0, 0.0, 3.0]; 3];
    mesh.uvs = vec![[0.0, 1.0]; 3];
    mesh.colors = vec![[1.0, 0.5, 0.25, 1.0]; 3];
    let (json, bin) = split(&write_glb(&single(vec![mesh], vec![shader(&[])]), "m", &|_| None));
    for key in ["POSITION", "NORMAL", "TEXCOORD_0", "COLOR_0"] {
        assert!(json.contains(&format!("\"{key}\":")), "{key} in {json}");
    }
    assert!(!json.contains("JOINTS_0"));
    // positions 36 bytes, normals 36, uvs 24, colours 48, indices 12
    assert_eq!(bin.len(), 36 + 36 + 24 + 48 + 12);
    assert_eq!(&bin[36..48], [0.0f32, 0.0, 1.0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(&bin[bin.len() - 12..], [0u32, 1, 2].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
}

#[test]
fn attributes_of_the_wrong_length_are_left_out() {
    let mut mesh = triangle_mesh(0);
    mesh.normals = vec![[0.0, 0.0, 1.0]; 2];
    mesh.uvs = vec![[0.0; 2]; 4];
    mesh.colors = vec![[1.0; 4]; 1];
    let (json, _) = split(&write_glb(&single(vec![mesh], vec![shader(&[])]), "m", &|_| None));
    assert!(json.contains("\"POSITION\"") && !json.contains("NORMAL") && !json.contains("TEXCOORD_0"));
    assert!(!json.contains("COLOR_0"));
}

#[test]
fn positions_that_are_not_numbers_are_written_as_zero() {
    let mut mesh = triangle_mesh(0);
    mesh.positions[1] = [f32::NAN, f32::INFINITY, 2.0];
    let (json, bin) = split(&write_glb(&single(vec![mesh], vec![shader(&[])]), "m", &|_| None));
    assert_eq!(&bin[12..24], [0.0f32, 0.0, 2.0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
    assert!(json.contains("\"min\":[0,0,0],\"max\":[0,1,2]"), "{json}");
}

#[test]
fn meshes_without_vertices_or_indices_are_left_out() {
    let empty_indices = Mesh { indices: Vec::new(), ..triangle_mesh(0) };
    let d = single(vec![Mesh::default(), empty_indices], vec![shader(&[])]);
    let (json, bin) = split(&write_glb(&d, "m", &|_| None));
    assert!(json.contains("\"meshes\":[]"), "{json}");
    assert!(bin.is_empty());
}

#[test]
fn a_mesh_with_a_shader_the_drawable_lacks_has_no_material() {
    let d = single(vec![triangle_mesh(5)], vec![shader(&[])]);
    let (json, _) = split(&write_glb(&d, "m", &|_| None));
    assert!(json.contains("\"primitives\":[{\"attributes\":{\"POSITION\":0},\"indices\":1}]"), "{json}");
}

#[test]
fn a_textured_material_embeds_the_texture_as_png() {
    let texture = rgba_texture("body");
    let d = single(vec![triangle_mesh(0)], vec![shader(&[(param::DIFFUSE_TEX, "body")])]);
    let glb = write_glb(&d, "car", &|name| (name == "body").then_some(&texture));
    let (json, bin) = split(&glb);
    assert!(json.contains("\"baseColorTexture\":{\"index\":0}"), "{json}");
    assert!(json.contains("\"images\":[{\"name\":\"body\",\"bufferView\":0,\"mimeType\":\"image/png\"}]"), "{json}");
    assert!(json.contains("\"textures\":[{\"source\":0,\"sampler\":0}]"), "{json}");
    assert!(json.contains("\"samplers\":[{\"magFilter\":9729"), "{json}");
    assert!(json.contains("\"name\":\"body\",\"pbrMetallicRoughness\""), "the material is named for its texture");
    let png = crate::png(2, 2, &texture.to_rgba().unwrap()).unwrap();
    assert!(bin.windows(png.len()).any(|w| w == png), "the PNG is in the binary chunk");
}

#[test]
fn a_texture_used_twice_is_embedded_once() {
    let texture = rgba_texture("body");
    let d = single(
        vec![triangle_mesh(0), triangle_mesh(1)],
        vec![shader(&[(param::DIFFUSE_TEX, "body")]), shader(&[(param::DIFFUSE_TEX, "BODY")])],
    );
    let (json, _) = split(&write_glb(&d, "car", &|_| Some(&texture)));
    assert_eq!(json.matches("\"mimeType\"").count(), 1);
    assert_eq!(json.matches("\"baseColorTexture\":{\"index\":0}").count(), 2);
}

#[test]
fn a_missing_texture_leaves_the_material_untextured() {
    let d = single(vec![triangle_mesh(0)], vec![shader(&[(param::DIFFUSE_TEX, "gone")])]);
    let (json, _) = split(&write_glb(&d, "car", &|_| None));
    assert!(!json.contains("baseColorTexture") && !json.contains("\"images\""), "{json}");
    assert!(json.contains("\"name\":\"gone\""), "still named for its texture: {json}");
}

#[test]
fn a_texture_that_cannot_be_decoded_leaves_the_material_untextured() {
    let texture = Texture { data: Vec::new(), ..rgba_texture("body") };
    let d = single(vec![triangle_mesh(0)], vec![shader(&[(param::DIFFUSE_TEX, "body")])]);
    let (json, _) = split(&write_glb(&d, "car", &|_| Some(&texture)));
    assert!(!json.contains("baseColorTexture"), "{json}");
}

#[test]
fn a_material_without_a_texture_is_named_after_the_drawable_and_shader() {
    let d = single(vec![triangle_mesh(1)], vec![shader(&[]), shader(&[])]);
    let (json, _) = split(&write_glb(&d, "van", &|_| None));
    assert!(json.contains("\"name\":\"van_shader_0\"") && json.contains("\"name\":\"van_shader_1\""), "{json}");
    assert!(json.contains("\"metallicFactor\":0,\"roughnessFactor\":0.8"));
    assert!(json.contains("\"alphaMode\":\"MASK\",\"alphaCutoff\":0.3,\"doubleSided\":true"));
}

#[test]
fn skin_shaders_are_opaque() {
    let skin = Shader { name_hash: rage_core::hash::joaat("gta_ped_skin"), ..Default::default() };
    let (json, _) = split(&write_glb(&single(vec![triangle_mesh(0)], vec![skin]), "ped", &|_| None));
    assert!(json.contains("\"alphaMode\":\"OPAQUE\",\"doubleSided\":true"), "{json}");
    assert!(!json.contains("alphaCutoff"));
}

#[test]
fn a_scene_places_drawables_with_their_materials() {
    let a = single(vec![triangle_mesh(0)], vec![shader(&[])]);
    let b = single(vec![triangle_mesh(0)], vec![shader(&[])]);
    let unused = single(vec![triangle_mesh(0)], vec![shader(&[])]);
    let moved = math::from_trs([5.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
    let nodes = [
        identity_node("first", 0),
        Node { name: "second \"b\"".into(), mesh: 1, transform: moved },
        identity_node("again", 0),
        identity_node("out of range", 9),
    ];
    let glb = write_glb_scene("scene", &[("a", &a), ("b", &b), ("unused", &unused)], &nodes, &|_, _| None, &|di, _| {
        (di != 1 || true).then_some([1.0; 4])
    });
    let (json, _) = split(&glb);
    assert_eq!(json.matches("\"primitives\"").count(), 2, "the drawable no node uses is left out: {json}");
    assert!(json.contains("\"children\":[1,2,3]"), "{json}");
    assert!(json.contains("{\"name\":\"first\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],\"mesh\":0}"), "{json}");
    assert!(
        json.contains("{\"name\":\"second \\\"b\\\"\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,5,0,0,1],\"mesh\":1}"),
        "{json}"
    );
    assert!(json.contains("{\"name\":\"again\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],\"mesh\":0}"), "{json}");
    assert_eq!(json.matches("\"materials\":[").count(), 1);
    assert_eq!(json.matches("\"pbrMetallicRoughness\"").count(), 2);
}

#[test]
fn a_material_callback_can_tint_or_drop_meshes() {
    let d = single(vec![triangle_mesh(0), triangle_mesh(1)], vec![shader(&[]), shader(&[])]);
    let nodes = [identity_node("n", 0)];
    let glb = write_glb_scene("scene", &[("d", &d)], &nodes, &|_, _| None, &|_, si| match si {
        0 => Some([0.5, f32::NAN, 1.0, 1.0]),
        _ => None,
    });
    let (json, _) = split(&glb);
    assert!(json.contains("\"baseColorFactor\":[0.5,0,1,1]"), "{json}");
    assert_eq!(json.matches("\"attributes\"").count(), 1, "the mesh with the refused shader is left out");
}

#[test]
fn a_node_whose_meshes_are_all_left_out_is_skipped() {
    let d = single(vec![triangle_mesh(0)], vec![shader(&[])]);
    let nodes = [identity_node("n", 0)];
    let glb = write_glb_scene("scene", &[("d", &d)], &nodes, &|_, _| None, &|_, _| None);
    let (json, _) = split(&glb);
    assert!(json.contains("\"children\":[]") && json.contains("\"meshes\":[]"), "{json}");
}

fn skinned_and_rigid(rig: bool) -> Drawable {
    let mut skinned = triangle_mesh(0);
    skinned.bone_indices = vec![[0, 1, 0, 0]; 3];
    skinned.bone_weights = vec![[0.5, 0.5, 0.0, 0.0]; 3];
    let skeleton = rig.then(|| Skeleton { bones: vec![bone("root", -1, [0.0; 3]), bone("tip", 0, [1.0, 0.0, 0.0])] });
    Drawable { meshes: vec![skinned], shaders: vec![shader(&[])], skeleton, ..Default::default() }
}

#[test]
fn a_skeleton_chosen_by_the_caller_skins_the_meshes() {
    let d = skinned_and_rigid(false);
    let rig = Skeleton { bones: vec![bone("root", -1, [0.0; 3]), bone("tip", 0, [1.0, 0.0, 0.0])] };
    let nodes = [identity_node("n", 0)];
    let glb =
        write_glb_scene_with_skins("s", &[("d", &d)], &nodes, &|_, _| None, &|_, _| Some([1.0; 4]), &|_| Some(&rig));
    let (json, _) = split(&glb);
    assert_eq!(json.matches("\"JOINTS_0\"").count(), 1, "{json}");
    assert!(json.contains("\"skins\":[{\"joints\":[3,4]"), "{json}");
    let none = write_glb_scene("s", &[("d", &d)], &nodes, &|_, _| None, &|_, _| Some([1.0; 4]));
    assert!(!split(&none).0.contains("JOINTS_0"), "without a skeleton the mesh is rigid");
}

#[test]
fn a_skeleton_without_bones_does_not_skin() {
    let d = skinned_and_rigid(false);
    let rig = Skeleton::default();
    let nodes = [identity_node("n", 0)];
    let glb =
        write_glb_scene_with_skins("s", &[("d", &d)], &nodes, &|_, _| None, &|_, _| Some([1.0; 4]), &|_| Some(&rig));
    let (json, _) = split(&glb);
    assert!(!json.contains("skins") && !json.contains("JOINTS_0"), "{json}");
}

#[test]
fn the_skin_of_a_node_hangs_below_its_placement() {
    let d = skinned_and_rigid(true);
    let (json, bin) = split(&write_glb(&d, "n", &|_| None));
    assert!(
        json.contains("{\"name\":\"n\",\"matrix\":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],\"children\":[2,3]}"),
        "{json}"
    );
    assert!(json.contains("{\"name\":\"n\",\"mesh\":0,\"skin\":0}"), "{json}");
    assert!(json.contains("\"inverseBindMatrices\":"));
    assert_eq!(bin.len() % 4, 0);
}

#[test]
fn the_binary_chunk_keeps_every_view_aligned() {
    let texture = rgba_texture("body");
    let d = single(vec![triangle_mesh(0)], vec![shader(&[(param::DIFFUSE_TEX, "body")])]);
    let (json, _) = split(&write_glb(&d, "m", &|_| Some(&texture)));
    let offsets: Vec<usize> = json
        .split("\"byteOffset\":")
        .skip(1)
        .map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap())
        .collect();
    assert!(offsets.len() >= 3);
    assert!(offsets.iter().all(|o| o % 4 == 0), "{offsets:?}");
}

#[test]
fn the_container_pads_its_json_with_spaces() {
    let json = glb_container(b"{}", &[1, 2, 3, 4]);
    assert_eq!(json.len(), 12 + 8 + 2 + 8 + 4);
    assert_eq!(&json[20..22], b"{}");
    let empty = glb_container(b"", &[]);
    assert_eq!(empty.len(), 28);
    assert_eq!(u32::from_le_bytes(empty[8..12].try_into().unwrap()), 28);
}
