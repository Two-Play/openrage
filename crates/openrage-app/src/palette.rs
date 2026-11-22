//! RDR2 palette shaders: colour textures that hold a shade, coloured from a
//! palette per *slot*. The slot of a vertex is its vertex colour's blue
//! channel (0, 1, 2, … as bytes); a mesh can mix slots.
//!
//! Two kinds, by the shader's textures (`lyr0DiffuseTex` holds the shade):
//!
//! - Vehicles (`standard_vehicle_2lyr`, … with `vehTintPaletteTex`, e.g.
//!   `generic_wagon_palette`): the palette has one row per colour (named in
//!   `carcols.ymt`), its columns run from dark to light. A vehicle's
//!   variation (`carvariations.ymt`) gives each slot a paint colour and a
//!   worn colour. The shade's green picks the column, its blue (scratches)
//!   blends the paint towards the worn colour.
//! - Props and weapons (`TintPaletteTex`, e.g. `w_repeater_cloth_strap01_pal`,
//!   256 × tints): column = slot, row = tint (0 here). The grey colour
//!   texture is laid over the palette colour (hard light).
//!
//! The game builds the palette texture of a vehicle at run time; the one in
//! its files (`Generic_skinned_pal`) is a debug palette and is not used here.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rage_formats::drawable::{Drawable, Mesh, Shader, param};
use rage_formats::texture::{Format, Texture};
use rage_formats::ytd::TextureDictionary;

/// Vehicle colour variations.
pub const CARVARIATIONS: &str = "packs_0.rpf/packs/base/data/carvariations.ymt";

/// The game's debug palette in vehicle shaders (replaced at run time).
const DEBUG_PALETTE: &str = "generic_skinned_pal";

/// A vehicle palette slot: rows of the vehicle palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VehicleSlot {
    pub paint: u8,
    pub worn: u8,
}

/// The slots of each vehicle's first colour variation, by model name (lower
/// case), from `carvariations.ymt` as XML.
pub fn parse_carvariations(xml: &str) -> Variations {
    // Field names are unknown: hashes.
    const VARIATIONS: &str = "hash_99C68B00";
    const SLOTS: &str = "hash_E0E9752E";
    const PAINT: &str = "hash_A2039C71";
    const WORN: &str = "hash_41F13680";
    let mut out = HashMap::new();
    let Ok(doc) = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() },
    ) else {
        return out;
    };
    fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        n.children().find(|c| c.has_tag_name(name))
    }
    let value = |n: roxmltree::Node<'_, '_>, name: &str| {
        child(n, name).and_then(|c| c.attribute("value")?.parse::<u32>().ok()).map(|v| v.min(255) as u8)
    };
    for model in doc.descendants().filter(|n| n.has_tag_name("Item")) {
        let Some(name) = child(model, "modelName").and_then(|n| n.text()) else { continue };
        let first = child(model, VARIATIONS).and_then(|v| v.children().find(|c| c.has_tag_name("Item")));
        let Some(slots) = first.and_then(|v| child(v, SLOTS)) else { continue };
        let slots: Vec<VehicleSlot> = slots
            .children()
            .filter(|c| c.has_tag_name("Item"))
            .map(|s| VehicleSlot { paint: value(s, PAINT).unwrap_or(0), worn: value(s, WORN).unwrap_or(0) })
            .collect();
        out.entry(name.trim().to_ascii_lowercase()).or_insert(slots);
    }
    out
}

/// Palette slots by vehicle model name.
pub type Variations = HashMap<String, Vec<VehicleSlot>>;

/// The vehicle variations of a game folder, read once.
pub fn carvariations(game: &Path) -> Arc<Variations> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Variations>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(game).cloned()) {
        return v;
    }
    let read = crate::metadata::load(game, None, CARVARIATIONS).map(|x| parse_carvariations(&x.text));
    let v = Arc::new(read.unwrap_or_default());
    if let Ok(mut c) = cache.lock() {
        c.insert(game.to_path_buf(), v.clone());
    }
    v
}

/// One RGBA pixel of a palette, columns interpolated (`x` in pixels).
fn palette_at(palette: &[u8], width: usize, height: usize, row: usize, x: f32) -> [f32; 3] {
    let row = row.min(height.saturating_sub(1));
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let (x0, t) = (x.floor() as usize, x.fract());
    let x1 = (x0 + 1).min(width - 1);
    let px = |x: usize, c: usize| palette.get((row * width + x) * 4 + c).copied().unwrap_or(0) as f32;
    [0, 1, 2].map(|c| px(x0, c) * (1.0 - t) + px(x1, c) * t)
}

/// Colours a vehicle shade texture (RGBA) with a slot's palette rows: green
/// picks the column, blue blends from the paint row to the worn row.
pub fn bake_vehicle(shade: &[u8], palette: &[u8], width: usize, height: usize, slot: VehicleSlot) -> Vec<u8> {
    let mut out = Vec::with_capacity(shade.len());
    if width == 0 || height == 0 {
        return shade.to_vec();
    }
    let ramp = (width - 1) as f32;
    for px in shade.as_chunks::<4>().0 {
        let x = px[1] as f32 / 255.0 * ramp;
        let paint = palette_at(palette, width, height, slot.paint as usize, x);
        let worn = palette_at(palette, width, height, slot.worn as usize, x);
        let t = px[2] as f32 / 255.0;
        out.extend([0, 1, 2].map(|c| (paint[c] * (1.0 - t) + worn[c] * t).round().clamp(0.0, 255.0) as u8));
        out.push(px[3]);
    }
    out
}

/// Lays a grey colour texture (RGBA) over a colour (hard light): mid grey
/// gives the colour, darker and lighter shades darken and lighten it.
pub fn bake_tint(grey: &[u8], colour: [f32; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(grey.len());
    for px in grey.as_chunks::<4>().0 {
        let g = px[1] as f32 / 255.0;
        out.extend(colour.map(|c| {
            let c = c / 255.0;
            let v = if g < 0.5 { 2.0 * g * c } else { 1.0 - 2.0 * (1.0 - g) * (1.0 - c) };
            (v * 255.0).round().clamp(0.0, 255.0) as u8
        }));
        out.push(px[3]);
    }
    out
}

/// A dictionary of textures made here (not read from a file).
fn dictionary(textures: Vec<Texture>) -> TextureDictionary {
    TextureDictionary {
        textures,
        errors: Vec::new(),
        layout: rage_formats::ytd::Layout::Rdr2,
        version: 0,
        root: Vec::new(),
        stored: Vec::new(),
    }
}

/// How a shader's palette is applied.
#[derive(Clone, Debug)]
enum Kind {
    Vehicle { palette: String },
    Tint { palette: String },
}

fn kind(shader: &Shader, vehicle: bool) -> Option<Kind> {
    shader.sampler(param::LYR0_DIFFUSE_TEX)?;
    if vehicle && let Some(p) = shader.sampler(param::VEH_TINT_PALETTE_TEX) {
        return Some(Kind::Vehicle { palette: p.to_owned() });
    }
    let p = shader.sampler(param::TINT_PALETTE_TEX)?;
    (!p.eq_ignore_ascii_case(DEBUG_PALETTE)).then(|| Kind::Tint { palette: p.to_owned() })
}

/// A mesh with only the given triangles (indices into `mesh`'s vertices).
fn submesh(mesh: &Mesh, indices: &[u32]) -> Mesh {
    let n = mesh.positions.len();
    let mut out = Mesh { shader: mesh.shader, bone_tags: mesh.bone_tags.clone(), ..Default::default() };
    let mut map: HashMap<u32, u32> = HashMap::new();
    for &i in indices {
        let k = *map.entry(i).or_insert_with(|| {
            let v = i as usize;
            fn copy<T: Copy>(dst: &mut Vec<T>, src: &[T], n: usize, v: usize) {
                if src.len() == n
                    && let Some(x) = src.get(v)
                {
                    dst.push(*x);
                }
            }
            copy(&mut out.positions, &mesh.positions, n, v);
            copy(&mut out.normals, &mesh.normals, n, v);
            copy(&mut out.uvs, &mesh.uvs, n, v);
            copy(&mut out.uvs1, &mesh.uvs1, n, v);
            copy(&mut out.colors, &mesh.colors, n, v);
            copy(&mut out.bone_indices, &mesh.bone_indices, n, v);
            copy(&mut out.bone_weights, &mesh.bone_weights, n, v);
            out.positions.len() as u32 - 1
        });
        out.indices.push(k);
    }
    out
}

/// The palette slot of a vertex (vertex colour blue as a byte).
fn slot_of(mesh: &Mesh, v: u32) -> u8 {
    mesh.colors.get(v as usize).map_or(0, |c| (c[2] * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Colours the palette shaders of `drawables`: each (shader, slot) gets a
/// baked colour texture (added to `external`) and a copy of the shader that
/// uses it; meshes that mix slots are split by slot. `model` names the
/// vehicle in `carvariations.ymt` (vehicles without one keep their shade).
pub fn apply(
    variations: &Variations,
    model: &str,
    drawables: &mut [(String, Drawable)],
    external: &mut Vec<(String, TextureDictionary)>,
) {
    let vehicle_slots = variations.get(&model.to_ascii_lowercase());
    let mut baked: Vec<Texture> = Vec::new();
    for (_, d) in drawables.iter_mut() {
        let kinds: Vec<Option<Kind>> = d.shaders.iter().map(|s| kind(s, vehicle_slots.is_some())).collect();
        if kinds.iter().all(Option::is_none) {
            continue;
        }
        // Split the meshes by slot.
        let mut meshes: Vec<(Mesh, Option<u8>)> = Vec::new();
        for mesh in std::mem::take(&mut d.meshes) {
            let paletted = kinds.get(mesh.shader as usize).is_some_and(Option::is_some);
            if !paletted || mesh.colors.len() != mesh.positions.len() {
                meshes.push((mesh, None));
                continue;
            }
            let mut by_slot: BTreeMap<u8, Vec<u32>> = BTreeMap::new();
            for tri in mesh.indices.chunks(3) {
                by_slot.entry(slot_of(&mesh, tri[0])).or_default().extend_from_slice(tri);
            }
            if by_slot.len() <= 1 {
                let slot = by_slot.keys().next().copied().unwrap_or(0);
                meshes.push((mesh, Some(slot)));
            } else {
                meshes.extend(by_slot.iter().map(|(&slot, tris)| (submesh(&mesh, tris), Some(slot))));
            }
        }
        // A baked texture and shader per (shader, slot).
        let mut made: HashMap<(u16, u8), Option<u16>> = HashMap::new();
        for (mesh, slot) in &mut meshes {
            let Some(slot) = *slot else { continue };
            let key = (mesh.shader, slot);
            let index = match made.get(&key) {
                Some(i) => *i,
                None => {
                    let i = bake_shader(d, &kinds, key, vehicle_slots, external, &mut baked);
                    made.insert(key, i);
                    i
                }
            };
            if let Some(i) = index {
                mesh.shader = i;
                // The vertex colour held the slot, not a colour.
                mesh.colors.clear();
            }
        }
        d.meshes = meshes.into_iter().map(|(m, _)| m).collect();
        drop_replaced(d, &kinds);
    }
    if baked.is_empty() {
        return;
    }
    external.push(("palettes (baked)".to_owned(), dictionary(baked)));
}

/// Removes the palette shaders that no mesh uses any more (all their meshes
/// now use baked copies), so exports carry no unused materials.
fn drop_replaced(d: &mut Drawable, kinds: &[Option<Kind>]) {
    let used = |i: usize| d.meshes.iter().any(|m| m.shader as usize == i);
    let keep: Vec<bool> = (0..d.shaders.len()).map(|i| kinds.get(i).is_none_or(Option::is_none) || used(i)).collect();
    if keep.iter().all(|k| *k) {
        return;
    }
    let mut new_index = Vec::with_capacity(keep.len());
    let mut next = 0u16;
    for &k in &keep {
        new_index.push(next);
        next += u16::from(k);
    }
    for m in &mut d.meshes {
        m.shader = new_index.get(m.shader as usize).copied().unwrap_or(m.shader);
    }
    let mut k = keep.iter();
    d.shaders.retain(|_| *k.next().unwrap_or(&true));
}

/// Bakes the colour texture of shader `key.0` for slot `key.1` and adds a
/// copy of the shader that uses it; its index, or `None` when the textures
/// are missing.
fn bake_shader(
    d: &mut Drawable,
    kinds: &[Option<Kind>],
    (shader, slot): (u16, u8),
    vehicle_slots: Option<&Vec<VehicleSlot>>,
    external: &[(String, TextureDictionary)],
    baked: &mut Vec<Texture>,
) -> Option<u16> {
    let kind = kinds.get(shader as usize)?.as_ref()?;
    let original = d.shaders.get(shader as usize)?;
    let shade_name = original.sampler(param::LYR0_DIFFUSE_TEX)?.to_owned();
    let (palette_name, tag) = match kind {
        Kind::Vehicle { palette } => {
            let s = vehicle_slots?.get(slot as usize)?;
            (palette.clone(), format!("{}_{}", s.paint, s.worn))
        }
        Kind::Tint { palette } => (palette.clone(), slot.to_string()),
    };
    let new_name = format!("{shade_name}+{palette_name}#{tag}");
    if !baked.iter().any(|t| t.name == new_name) {
        let find = |name: &str| {
            let f = |t: &TextureDictionary| t.textures.iter().find(|x| x.name.eq_ignore_ascii_case(name)).cloned();
            d.textures.as_ref().and_then(f).or_else(|| external.iter().find_map(|(_, t)| f(t)))
        };
        let shade = find(&shade_name)?;
        let palette = find(&palette_name)?;
        let (rgba, pal) = (shade.to_rgba()?, palette.to_rgba()?);
        let (w, h) = (palette.width.max(1) as usize, palette.height.max(1) as usize);
        let data = match kind {
            Kind::Vehicle { .. } => bake_vehicle(&rgba, &pal, w, h, *vehicle_slots?.get(slot as usize)?),
            Kind::Tint { .. } => bake_tint(&rgba, palette_at(&pal, w, h, 0, slot as f32)),
        };
        baked.push(Texture {
            name: new_name.clone(),
            width: shade.width.max(1),
            height: shade.height.max(1),
            depth: 1,
            levels: 1,
            format: Format::R8G8B8A8_UNORM,
            dimension: shade.dimension,
            usage: shade.usage,
            data,
        });
    }
    // Slots with the same colours share one copy.
    if let Some(i) = d.shaders.iter().position(|s| s.sampler(param::LYR0_DIFFUSE_TEX) == Some(new_name.as_str())) {
        return u16::try_from(i).ok();
    }
    let mut copy = original.clone();
    for (h, t) in copy.samplers.iter_mut() {
        if *h == param::LYR0_DIFFUSE_TEX {
            *t = new_name.clone();
        }
    }
    copy.textures.push(new_name);
    d.shaders.push(copy);
    u16::try_from(d.shaders.len() - 1).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_first_variation_slots() {
        let xml = r#"<CVehicleModelInfoVariation><variationData>
          <Item><modelName>cart07</modelName>
            <hash_99C68B00>
              <Item><hash_E0E9752E>
                <Item><hash_A2039C71 value="8" /><hash_41F13680 value="242" /></Item>
                <Item><hash_A2039C71 value="22" /><hash_41F13680 value="254" /></Item>
              </hash_E0E9752E></Item>
              <Item><hash_E0E9752E>
                <Item><hash_A2039C71 value="13" /><hash_41F13680 value="244" /></Item>
              </hash_E0E9752E></Item>
            </hash_99C68B00>
          </Item></variationData></CVehicleModelInfoVariation>"#;
        let v = parse_carvariations(xml);
        assert_eq!(v["cart07"], vec![VehicleSlot { paint: 8, worn: 242 }, VehicleSlot { paint: 22, worn: 254 }]);
    }

    #[test]
    fn vehicle_shade_picks_column_scratches_pick_worn_row() {
        // Two rows, 3 wide: row 0 ramps red, row 1 is blue.
        let pal = [0, 0, 0, 255, 100, 0, 0, 255, 200, 0, 0, 255, 0, 0, 90, 255, 0, 0, 90, 255, 0, 0, 90, 255];
        let slot = VehicleSlot { paint: 0, worn: 1 };
        assert_eq!(bake_vehicle(&[0, 255, 0, 7], &pal, 3, 2, slot), vec![200, 0, 0, 7]);
        assert_eq!(bake_vehicle(&[0, 128, 0, 255], &pal, 3, 2, slot), vec![100, 0, 0, 255]);
        assert_eq!(bake_vehicle(&[0, 0, 255, 255], &pal, 3, 2, slot), vec![0, 0, 90, 255]);
    }

    #[test]
    fn hard_light_keeps_colour_at_mid_grey() {
        let c = [200.0, 100.0, 50.0];
        assert_eq!(bake_tint(&[0, 0, 0, 255], c), vec![0, 0, 0, 255]);
        assert_eq!(bake_tint(&[255, 255, 255, 255], c), vec![255, 255, 255, 255]);
        let mid = bake_tint(&[128, 128, 128, 255], c);
        assert!(mid[..3].iter().zip(c).all(|(a, b)| (*a as f32 - b).abs() <= 2.0), "{mid:?}");
    }

    #[test]
    fn splits_meshes_by_slot() {
        let shader = |pal: &str| Shader {
            name_hash: 1,
            textures: vec!["shade".into(), pal.into()],
            samplers: vec![(param::LYR0_DIFFUSE_TEX, "shade".into()), (param::TINT_PALETTE_TEX, pal.into())],
        };
        let tex = |name: &str, w: u32, h: u32, data: Vec<u8>| Texture {
            name: name.into(),
            width: w,
            height: h,
            depth: 1,
            levels: 1,
            format: Format::R8G8B8A8_UNORM,
            dimension: rage_formats::texture::Dimension::Texture2D,
            usage: 0,
            data,
        };
        // Slot 0 red, slot 1 green.
        let pal = tex("thing_pal", 2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        let shade = tex("shade", 1, 1, vec![128, 128, 128, 255]);
        let blue = |b: f32| [1.0, 1.0, b, 1.0];
        let mesh = Mesh {
            positions: vec![[0.0; 3]; 6],
            colors: vec![blue(0.0), blue(0.0), blue(0.0), blue(1.0 / 255.0), blue(1.0 / 255.0), blue(1.0 / 255.0)],
            indices: vec![0, 1, 2, 3, 4, 5],
            ..Default::default()
        };
        let d = Drawable {
            meshes: vec![mesh],
            shaders: vec![shader("thing_pal"), shader(DEBUG_PALETTE)],
            textures: Some(dictionary(vec![pal, shade])),
            ..Default::default()
        };
        let mut drawables = vec![("thing".to_owned(), d)];
        let mut external = Vec::new();
        apply(&HashMap::new(), "thing", &mut drawables, &mut external);
        let d = &drawables[0].1;
        assert_eq!(d.meshes.len(), 2);
        // The replaced shader is gone: the debug one and two copies are left.
        assert_eq!(d.shaders.len(), 3);
        assert!(d.meshes.iter().all(|m| m.positions.len() == 3 && m.colors.is_empty()));
        let baked = &external[0].1.textures;
        let colour = |m: &Mesh| {
            let name = d.shaders[m.shader as usize].base_texture().unwrap();
            baked.iter().find(|t| t.name == name).unwrap().data[..3].to_vec()
        };
        assert_eq!(colour(&d.meshes[0]), vec![255, 1, 1]);
        assert_eq!(colour(&d.meshes[1]), vec![1, 255, 1]);
        // The debug palette is left alone: its shade stays the colour.
        assert_eq!(d.shaders[0].sampler(param::TINT_PALETTE_TEX), Some(DEBUG_PALETTE));
        assert_eq!(d.shaders[0].base_texture(), Some("shade"));
    }
}
