//! GTA VI models.
//!
//! A model is stored as its system part and then its graphics part. The
//! archive's table says how long they are. In the recovery view both are cut into zlib streams of 64 KiB. Without the table of contents nothing
//! says where one part ends: the recovery view lists a resource as its first
//! entry (which starts the system part) and what follows it. So the parts
//! are found by trying: every cut between two streams is a possible end of
//! the system part, and the reader says whether the graphics part reaches
//! far enough.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use rage_formats::drawable::{Bone, Drawable, Skeleton};
use rage_formats::drawable6::{self, ModelError};
use rage_formats::math;
use rage_rpf::Rpf9;

/// Streams unpack to this many bytes, the last one of a part to fewer.
const STREAM: usize = 0x10000;
/// A system part is at most this long.
const MAX_SYSTEM: usize = 32 << 20;
/// A resource is at most this long, and spread over at most this many entries.
const MAX_RESOURCE: usize = 256 << 20;
const MAX_ENTRIES: usize = 4096;

/// Reads the model that starts at entry `index`: its drawables with their names.
pub fn load(rpf: &Rpf9, index: usize) -> Result<Vec<(String, Drawable)>> {
    let entries = rpf.entries()?;
    let first = entries.get(index).context("no such entry")?;
    let mut data = rpf.read(first)?;
    if drawable6::root(&data).is_none() {
        bail!("this resource is not a model");
    }
    if let Some((system, _)) = rpf.resource_parts(first) {
        let (system, graphics) = data.split_at_checked(system as usize).context("the resource is cut off")?;
        return Ok(named(drawable6::parse(system, graphics)?));
    }
    // Where streams end: the possible ends of the system part.
    let mut cuts: Vec<usize> = (1..=data.len().div_ceil(STREAM)).map(|i| (i * STREAM).min(data.len())).collect();
    let mut last = index;
    loop {
        let mut needs_more = false;
        let mut failure = None;
        for cut in cuts.iter().copied().take_while(|c| *c <= MAX_SYSTEM) {
            match drawable6::parse(&data[..cut], &data[cut..]) {
                Ok(drawables) => return Ok(named(drawables)),
                Err(ModelError::Graphics(_)) => needs_more = true,
                Err(e) => failure = failure.or(Some(e)),
            }
        }
        // The graphics part goes on in the next entry.
        if !needs_more || !Rpf9::follows(&entries, last) || last - index >= MAX_ENTRIES || data.len() >= MAX_RESOURCE {
            match failure {
                Some(e) if !needs_more => bail!(e),
                _ => bail!("the model's vertex and index data was not found after it"),
            }
        }
        last += 1;
        let more = rpf.read(&entries[last])?;
        let start = data.len();
        data.extend_from_slice(&more);
        cuts.extend((1..=more.len().div_ceil(STREAM)).map(|i| start + (i * STREAM).min(more.len())));
    }
}

fn named(drawables: Vec<Drawable>) -> Vec<(String, Drawable)> {
    drawables
        .into_iter()
        .enumerate()
        .map(|(i, d)| (if d.name.is_empty() { format!("drawable {i}") } else { d.name.clone() }, d))
        .collect()
}

/// The archive with the skeletons people and animals are put together on.
const BASE_SKELETONS: &str = "packs.rpf/packs/base/models/metaped_fragments.rpf";
/// Its canonical female body skeleton identifies this archive when its nested name is unavailable.
const BASE_SKELETON_MARKER: &str = "ambient_femalebody.#ft";

/// A vertex with less weight than this on known bones moves with the nearest.
const MIN_WEIGHT: f32 = 0.3;

type Skeletons = Arc<Vec<(String, Skeleton)>>;

/// The base skeletons of the game with their file names (read once).
fn base_skeletons(game: &Path) -> Skeletons {
    static CACHE: Mutex<Option<(PathBuf, Skeletons)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((of, found)) = &*cache
        && of == game
    {
        return Arc::clone(found);
    }
    let found = Arc::new(rage_rpf::in_own_threads(|| read_base_skeletons(game)).unwrap_or_default());
    *cache = Some((game.to_path_buf(), Arc::clone(&found)));
    found
}

fn read_base_skeletons(game: &Path) -> Result<Vec<(String, Skeleton)>> {
    use rayon::prelude::*;
    let rpf = match rage_mods::resolve(game, BASE_SKELETONS, None) {
        Ok((rage_rpf::vfs::Node::Rpf9Dir(rpf, _), _)) => rpf,
        Err(rage_rpf::RpfError::NotFound(_)) => recovered_base_archive(game)?,
        Ok(_) => bail!("{BASE_SKELETONS} is not an archive"),
        Err(e) => return Err(e.into()),
    };
    let entries = rpf.entries()?;
    let mut found: Vec<(String, Skeleton)> = (0..entries.len())
        .into_par_iter()
        .filter_map(|i| {
            let name = rpf.entry_name(&entries[i]).to_ascii_lowercase();
            let (system, graphics) = crate::rpf9::resource(&rpf, i).ok()?;
            let skeleton = drawable6::parse(&system, &graphics).ok()?.into_iter().next()?.skeleton?;
            Some((name.rsplit('/').next().unwrap_or(&name).to_owned(), skeleton))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// Finds the base archive by content when its TOC is readable but its nested
/// path is unknown. If several archives match, the parallel search picks one.
fn recovered_base_archive(game: &Path) -> Result<Arc<Rpf9>> {
    use rayon::prelude::*;

    let (rage_rpf::vfs::Node::Rpf9Dir(packs, _), _) = rage_mods::resolve(game, "packs.rpf", None)? else {
        bail!("no packs archive");
    };
    packs
        .entries()?
        .par_iter()
        .filter(|entry| packs.is_archive(entry))
        .find_map_any(|entry| {
            let child = packs.open_child(entry).ok()?;
            let entries = child.entries().ok()?;
            let has_body = entries.iter().any(|entry| {
                let path = child.entry_name(entry);
                path.rsplit('/').next().is_some_and(|name| name.eq_ignore_ascii_case(BASE_SKELETON_MARKER))
            });
            has_body.then(|| Arc::new(child))
        })
        .context("base skeleton archive not found in packs.rpf")
}

/// Binds meshes that name bones of another skeleton (parts of people and
/// animals) to it, and gives that skeleton. `None` if there are none, or the
/// game has no skeleton for them.
pub(crate) fn bind(game: &Path, file: &str, drawables: &mut [(String, Drawable)]) -> Option<Skeleton> {
    let wanted: HashSet<u32> =
        drawables.iter().flat_map(|(_, d)| &d.meshes).flat_map(|m| &m.bone_hashes).copied().collect();
    if wanted.is_empty() {
        return None;
    }
    let bases = base_skeletons(game);
    let mut skeleton = best_base(&bases, file, &wanted)?.clone();
    bind_to(&mut skeleton, drawables);
    Some(skeleton)
}

/// The skeleton with the most of the bones; of several with as many, the one
/// whose name is most like the file's. `None` if none has any of them, and
/// the file's name does not tell either.
fn best_base<'a>(bases: &'a [(String, Skeleton)], file: &str, wanted: &HashSet<u32>) -> Option<&'a Skeleton> {
    let file = file.to_ascii_lowercase();
    let words: Vec<&str> = file.split(['_', '.', '+']).collect();
    // `fr1`, `mo1`, …: a woman's or a man's part, of one of the builds.
    let sex = words.iter().find_map(|w| match w.as_bytes() {
        [b'f', b'a'..=b'z', b'0'..=b'9'] => Some("ambient_femalebody.#ft"),
        [b'm', b'a'..=b'z', b'0'..=b'9'] => Some("ambient_malebody.#ft"),
        _ => None,
    });
    let rated = bases.iter().filter_map(|(name, skeleton)| {
        let own: HashSet<u32> = skeleton.bones.iter().map(|b| rage_core::hash::joaat(&b.name)).collect();
        let has = wanted.iter().filter(|h| own.contains(h)).count();
        let alike = words.iter().filter(|w| w.len() > 2 && name.contains(**w)).count();
        (has > 0 || sex == Some(name)).then_some(((has, sex == Some(name), alike), skeleton))
    });
    // The first one of the best.
    rated.reduce(|best, next| if next.0 > best.0 { next } else { best }).map(|(_, s)| s)
}

/// Binds the meshes to the skeleton, which gets the bones it lacks from the
/// drawables' own (hair, garments).
fn bind_to(skeleton: &mut Skeleton, drawables: &mut [(String, Drawable)]) {
    let places =
        |s: &Skeleton| -> Vec<[f32; 3]> { s.world_transforms().iter().map(|m| [m[12], m[13], m[14]]).collect() };
    // The main bones of the body, with their places.
    let body: Vec<(usize, [f32; 3])> = places(skeleton)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| skeleton.bones[*i].name.starts_with("SKEL_"))
        .collect();
    let nearest = |at: &[f32; 3]| -> Option<usize> {
        let far = |p: &[f32; 3]| (0..3).map(|k| (p[k] - at[k]).powi(2)).sum::<f32>();
        body.iter().min_by(|a, b| far(&a.1).total_cmp(&far(&b.1))).map(|(i, _)| *i)
    };
    // Where the bones of each drawable's own skeleton are.
    let mut of_own: Vec<Vec<i16>> = Vec::new();
    for (_, d) in drawables.iter() {
        let Some(own) = &d.skeleton else {
            of_own.push(Vec::new());
            continue;
        };
        let at = places(own);
        let mut placed: Vec<i16> = Vec::with_capacity(own.bones.len());
        for (i, b) in own.bones.iter().enumerate() {
            if let Some(there) = skeleton.find_hash(rage_core::hash::joaat(&b.name)) {
                placed.push(there as i16);
                continue;
            }
            let mut bone = b.clone();
            bone.parent = match usize::try_from(b.parent).ok().and_then(|p| placed.get(p)) {
                Some(&p) => p,
                // Loose: it hangs from the nearest bone of the body.
                None => {
                    let Some(nearest) = nearest(&at[i]) else {
                        placed.push(0);
                        continue;
                    };
                    attach(&mut bone, skeleton, nearest);
                    nearest as i16
                }
            };
            placed.push(skeleton.bones.len() as i16);
            skeleton.bones.push(bone);
        }
        of_own.push(placed);
    }
    for (k, (_, d)) in drawables.iter_mut().enumerate() {
        for mesh in &mut d.meshes {
            if mesh.bone_hashes.is_empty() {
                // Bound to the drawable's own skeleton.
                let own = &of_own[k];
                let there = |b: u16| own.get(usize::from(b)).map_or(b, |&b| b as u16);
                mesh.bone_indices.iter_mut().flatten().for_each(|b| *b = there(*b));
                continue;
            }
            let map: Vec<Option<u16>> =
                mesh.bone_hashes.iter().map(|&h| skeleton.find_hash(h).map(|b| b as u16)).collect();
            mesh.bone_hashes.clear();
            let vertices = mesh.positions.iter().zip(mesh.bone_indices.iter_mut().zip(&mut mesh.bone_weights));
            for (position, (bones, weights)) in vertices {
                // Bones of yet another skeleton do not count.
                for (b, w) in bones.iter_mut().zip(weights.iter_mut()) {
                    match map.get(usize::from(*b)).copied().flatten() {
                        Some(there) => *b = there,
                        None => (*b, *w) = (0, 0.0),
                    }
                }
                let total: f32 = weights.iter().sum();
                if total > MIN_WEIGHT {
                    weights.iter_mut().for_each(|w| *w /= total);
                } else {
                    *bones = [nearest(position).unwrap_or(0) as u16, 0, 0, 0];
                    *weights = [1.0, 0.0, 0.0, 0.0];
                }
            }
        }
    }
}

/// Moves a bone without a parent under one of the skeleton's, where it stays.
fn attach(bone: &mut Bone, skeleton: &Skeleton, parent: usize) {
    let inverse = math::inverse_affine(&skeleton.world_transforms()[parent]);
    bone.translation = math::transform_point(&inverse, bone.translation);
    let (mut turn, mut at) = ([0.0, 0.0, 0.0, 1.0], Some(parent));
    while let Some(b) = at.and_then(|i| skeleton.bones.get(i)) {
        turn = turned(b.rotation, turn);
        at = usize::try_from(b.parent).ok();
    }
    let back = [-turn[0], -turn[1], -turn[2], turn[3]];
    bone.rotation = turned(back, bone.rotation);
}

/// The rotation `b`, then `a`.
fn turned(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rage_core::hash::joaat;
    use rage_formats::drawable::Mesh;

    fn bone(name: &str, parent: i16, translation: [f32; 3], rotation: [f32; 4]) -> Bone {
        Bone { name: name.into(), tag: 0, parent, translation, rotation, scale: [1.0; 3] }
    }

    fn body() -> Skeleton {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        Skeleton {
            bones: vec![
                bone("SKEL_ROOT", -1, [0.0, 0.0, 1.0], [0.0, 0.0, half, half]),
                bone("SKEL_Head", 0, [0.0, 0.0, 0.7], [half, 0.0, 0.0, half]),
            ],
        }
    }

    #[test]
    fn parts_find_their_bones_by_name() {
        let mesh = Mesh {
            positions: vec![[0.0; 3], [0.0, 0.0, 1.8]],
            bone_indices: vec![[0, 1, 2, 9], [2, 0, 0, 0]],
            bone_weights: vec![[0.25, 0.25, 0.5, 0.0], [1.0, 0.0, 0.0, 0.0]],
            bone_hashes: vec![joaat("GM_Collar"), joaat("SKEL_ROOT"), joaat("unknown")],
            ..Default::default()
        };
        let own = Skeleton { bones: vec![bone("GM_Collar", -1, [0.1, 0.2, 1.6], [0.0, 0.0, 0.0, 1.0])] };
        let mut drawables =
            vec![("part".into(), Drawable { meshes: vec![mesh], skeleton: Some(own), ..Default::default() })];
        let mut skeleton = body();
        bind_to(&mut skeleton, &mut drawables);
        // Unknown bones do not count; what has no others moves with the nearest.
        let mesh = &drawables[0].1.meshes[0];
        assert_eq!(mesh.bone_indices, [[2, 0, 0, 0], [1, 0, 0, 0]]);
        assert_eq!(mesh.bone_weights, [[0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]]);
        // The garment's bone stays where it was.
        assert_eq!(skeleton.bones[2].parent, 1);
        let world = skeleton.world_transforms()[2];
        let expected = math::from_trs([0.1, 0.2, 1.6], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        assert!(world.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-5), "{world:?}");
    }

    #[test]
    fn the_skeleton_is_the_one_with_the_bones() {
        let dog = Skeleton { bones: vec![bone("SKEL_Tail", -1, [0.0; 3], [0.0, 0.0, 0.0, 1.0])] };
        let bases = [
            ("ambient_femalebody.#ft".to_owned(), body()),
            ("ambient_malebody.#ft".to_owned(), body()),
            ("crm_dog_00.#ft".to_owned(), dog),
        ];
        let pick = |file: &str, bones: &[&str]| {
            let found = best_base(&bases, file, &bones.iter().map(|b| joaat(b)).collect());
            found.and_then(|s| bases.iter().position(|(_, b)| std::ptr::eq(b, s)))
        };
        assert_eq!(pick("shirt_mr1_000.#dd", &["SKEL_Head"]), Some(1));
        assert_eq!(pick("shirt_fr1_000.#dd", &["SKEL_Head"]), Some(0));
        assert_eq!(pick("fur.#dd", &["SKEL_Tail"]), Some(2));
        assert_eq!(pick("fur.#dd", &["a", "b", "SKEL_Tail"]), Some(2));
        assert_eq!(pick("fur.#dd", &["a"]), None);
        assert_eq!(pick("belt_fr1_000.#dd", &["a"]), Some(0));
    }
}
