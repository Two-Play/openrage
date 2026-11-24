//! GTA VI clip dictionaries (`.#cd`): animations of skeletons.
//!
//! A descendant of Red Dead Redemption 2's layout (see [`crate::clip`]), with
//! names by hash (Jenkins one-at-a-time of the lower-case name) instead of bone
//! tags. Pointers are 64-bit addresses, from `0x5000_0000` into the system part.
//!
//! * dictionary: name `0x38`, clip hash buckets `0x48` (count `0x50`),
//!   animation dictionaries `0x70` (count `0x78`, 16 bytes each, a pointer
//!   first); bucket entries chain through `0x10`, object at `0x08`;
//! * animation dictionary: animation hash buckets `0x48` (count `0x50`);
//! * clip: kind `0x20` (2), name `0x28` (a pointer, or up to 24 bytes of
//!   text in place), parts `0x68` (count `0x70`), each 24 bytes: its layers
//!   (pointer, count at 8) and its duration at `0x10`;
//! * layer, `0x38` bytes: its animation, then start, end and rate at `0x28`.
//!   A clip's layers animate different tracks at different frame rates (the
//!   body at 30 frames a second, fingers and helper bones at 15 and 10) and
//!   play at the same time. They may be a section of a longer animation;
//! * animation: flags `0x20`, frames `0x24`, frames per block `0x26`,
//!   duration `0x28`, sequence blocks `0x50` (count `0x58`, pointers), tracks
//!   `0x60` (count `0x68`), each 12 bytes: the hash of the bone's name, the
//!   hash of what is animated (`boneRotation`, …) and the format (0
//!   quaternion, 2 vector, 3 two numbers, 4 one number). A block's last frame
//!   is the next one's first;
//! * sequence block: RDR2's, with eleven channel kinds. A track is a static
//!   quaternion (kind 0), a static vector (1), or a channel per component:
//!   static (2), raw (3), quantized (4), from a table (5) or packed (10). Kind 7
//!   makes the component its low bits say of a quaternion from the other
//!   three; kind 8 marks one with all four stored.
//!
//! Found by looking at the files themselves. What holds for every file read:
//! the channels cover each track exactly, the parameters end where the frames
//! start, packed channels end at their next chunk's start value, and
//! the last frame of a block is the first of the next. A block that does not
//! fit is an error, not a guess.
//!
//! Not known: the tags and properties of dictionaries and clips, the hashes
//! of most tracks that move no bone (movers, cameras, …), the data some blocks
//! have after their channel list (no track needs it), and
//! the kinds of channels no file seen has (6 and 9).

use std::collections::{HashMap, HashSet};

use rage_resource::{Resource, ResourceError};

use crate::clip::{
    Animation, BoneTrack, Clip, ClipDictionary, ClipPart, Layout, TRACK_BONE_ROTATION, TRACK_BONE_SCALE,
    TRACK_BONE_TRANSLATION, TRACK_OTHER, bad, map_objects, read_block,
};

/// `boneTranslation`, `boneRotation` and `boneScale`.
const BONE_TRANSLATION: u32 = 0xF791_A95F;
const BONE_ROTATION: u32 = 0x98E1_750C;
const BONE_SCALE: u32 = 0x7AAA_9F60;

/// The kind of clip with a list of parts, the only one seen.
const CLIP_PARTS: u32 = 2;
const PART_SIZE: u64 = 0x18;
const LAYER_SIZE: u64 = 0x38;
/// Longest clip that is put together, in frames.
const MAX_FRAMES: f32 = 1_000_000.0;

/// What a track animates, by its name's hash (`None` when the name is not known).
pub fn kind_name(hash: u32) -> Option<&'static str> {
    Some(match hash {
        BONE_TRANSLATION => "boneTranslation",
        BONE_ROTATION => "boneRotation",
        BONE_SCALE => "boneScale",
        0x2DEE_FADD => "genericControl",
        0xF5C3_A5B6 => "genericTranslation",
        0x6DDE_F121 => "cameraTranslation",
        _ => return None,
    })
}

fn float(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    let v = f32::from_bits(res.u32(at)?);
    if v.is_finite() { Ok(v) } else { Err(bad("a number that is none")) }
}

fn read_animation(res: &Resource, ptr: u64) -> Result<Animation, ResourceError> {
    let frames = usize::from(res.u16(ptr + 0x24)?);
    let duration = float(res, ptr + 0x28)?;
    let (blocks_at, block_count) = (res.u64(ptr + 0x50)?, u64::from(res.u16(ptr + 0x58)?));
    let (tracks_at, track_count) = (res.u64(ptr + 0x60)?, u64::from(res.u16(ptr + 0x68)?));
    if frames == 0 || !(0.0..1e5).contains(&duration) || block_count == 0 || block_count > 4096 {
        return Err(bad("implausible animation"));
    }
    let tracks: Vec<BoneTrack> = (0..track_count)
        .map(|i| {
            let at = tracks_at + i * 12;
            let kind_hash = res.u32(at + 4)?;
            let format = match res.u32(at + 8)? {
                0 => 1,
                2..=4 => 0,
                f => return Err(bad(format!("a track of unknown format {f}"))),
            };
            let track = match kind_hash {
                BONE_TRANSLATION => TRACK_BONE_TRANSLATION,
                BONE_ROTATION => TRACK_BONE_ROTATION,
                BONE_SCALE => TRACK_BONE_SCALE,
                _ => TRACK_OTHER,
            };
            Ok(BoneTrack { bone: 0, format, track, bone_hash: res.u32(at)?, kind_hash })
        })
        .collect::<Result<_, ResourceError>>()?;
    let blocks = (0..block_count)
        .map(|i| read_block(res, res.u64(blocks_at + i * 8)?, tracks.len(), Layout::GtaVi))
        .collect::<Result<Vec<_>, _>>()?;
    let animation =
        Animation { flags: 0, frames, frames_per_block: usize::from(res.u16(ptr + 0x26)?), duration, tracks, blocks };
    let stored: usize = animation.block_frames().iter().sum();
    if stored != frames + animation.blocks.len() - 1 {
        return Err(bad("an animation whose blocks have another number of frames"));
    }
    Ok(animation)
}

/// A clip's name: a pointer, or text in place. Without path and extension.
fn clip_name(res: &Resource, at: u64) -> Result<String, ResourceError> {
    let pointer = res.u64(at)?;
    let name = if pointer >= Resource::ROOT && res.bytes(pointer, 1).is_ok() {
        res.string(pointer)?
    } else {
        let text = res.bytes(at, 24)?;
        let len = text.iter().position(|b| *b == 0).unwrap_or(text.len());
        String::from_utf8_lossy(&text[..len]).into_owned()
    };
    let name = name.strip_prefix("pack:/").unwrap_or(&name);
    Ok(name.strip_suffix(".clip").unwrap_or(name).to_owned())
}

/// The layers of a part as one animation: the tracks of all of them (the
/// first's where two have the same), at the frame rate of the finest one.
fn join_layers(layers: &[ClipPart], animations: &[Animation], duration: f32) -> Result<Animation, ResourceError> {
    let rate = layers
        .iter()
        .map(|l| (&animations[l.animation], l.rate))
        .filter(|(a, _)| a.duration > 0.0)
        .map(|(a, rate)| (a.frames - 1) as f32 / a.duration * rate)
        .fold(0.0, f32::max);
    let frames = (duration * rate).round();
    if !(0.0..MAX_FRAMES).contains(&frames) {
        return Err(bad("implausible clip"));
    }
    let frames = frames as usize + 1;
    let mut seen = HashSet::new();
    let (mut tracks, mut values) = (Vec::new(), Vec::new());
    for layer in layers {
        let a = &animations[layer.animation];
        for (t, track) in a.tracks.iter().enumerate() {
            if !seen.insert((track.bone_hash, track.kind_hash)) {
                continue;
            }
            tracks.push(*track);
            let value = |f: usize| {
                let time = duration * f as f32 / (frames - 1).max(1) as f32;
                a.value(t, a.frame_at(layer.start + time * layer.rate)).unwrap_or_default()
            };
            values.push((0..frames).map(value).collect());
        }
    }
    Ok(Animation::from_frames(frames, duration, tracks, values))
}

/// Parses a clip dictionary.
pub fn parse(res: &Resource) -> Result<ClipDictionary, ResourceError> {
    let root = Resource::ROOT;
    if res.u64(root)? != 0 || res.string(res.u64(root + 0x38)?)?.is_empty() {
        return Err(bad("not a GTA VI clip dictionary"));
    }
    let mut stored: Vec<Animation> = Vec::new();
    let mut by_pointer: HashMap<u64, usize> = HashMap::new();
    let dicts = res.u64(root + 0x70)?;
    for d in 0..u64::from(res.u16(root + 0x78)?) {
        let dict = res.u64(dicts + d * 16)?;
        for at in map_objects(res, res.u64(dict + 0x48)?, u64::from(res.u16(dict + 0x50)?))? {
            if let std::collections::hash_map::Entry::Vacant(e) = by_pointer.entry(at) {
                e.insert(stored.len());
                stored.push(read_animation(res, at)?);
            }
        }
    }

    let mut out = ClipDictionary::default();
    // Where an animation, or a set of layers, is in `out`.
    let mut placed: HashMap<Vec<(usize, [u32; 3])>, usize> = HashMap::new();
    let mut used = vec![false; stored.len()];
    for clip in map_objects(res, res.u64(root + 0x48)?, u64::from(res.u16(root + 0x50)?))? {
        let name = clip_name(res, clip + 0x28)?;
        let kind = res.u32(clip + 0x20)?;
        if kind != CLIP_PARTS {
            return Err(bad(format!("{name}: a clip of unknown kind {kind}")));
        }
        let list = res.u64(clip + 0x68)?;
        let mut parts = Vec::new();
        for p in 0..u64::from(res.u16(clip + 0x70)?) {
            let part = list + p * PART_SIZE;
            let (layers_at, count) = (res.u64(part)?, u64::from(res.u16(part + 8)?));
            let duration = float(res, part + 0x10)?;
            let mut layers = Vec::new();
            for l in 0..count {
                let at = layers_at + l * LAYER_SIZE;
                let Some(&animation) = by_pointer.get(&res.u64(at)?) else {
                    return Err(bad(format!("{name}: a layer without its animation")));
                };
                used[animation] = true;
                let (start, end, rate) = (float(res, at + 0x28)?, float(res, at + 0x2C)?, float(res, at + 0x30)?);
                layers.push(ClipPart { animation, start, end, rate });
            }
            match layers.as_slice() {
                [] => {}
                [one] => {
                    let key = vec![(one.animation, [0; 3])];
                    let animation = *placed.entry(key).or_insert_with(|| {
                        out.animations.push(stored[one.animation].clone());
                        out.animations.len() - 1
                    });
                    parts.push(ClipPart { animation, ..one.clone() });
                }
                _ => {
                    let key: Vec<_> = layers
                        .iter()
                        .map(|l| (l.animation, [l.start.to_bits(), l.end.to_bits(), l.rate.to_bits()]))
                        .collect();
                    let animation = match placed.get(&key) {
                        Some(&i) => i,
                        None => {
                            out.animations.push(join_layers(&layers, &stored, duration)?);
                            placed.insert(key, out.animations.len() - 1);
                            out.animations.len() - 1
                        }
                    };
                    parts.push(ClipPart { animation, start: 0.0, end: duration, rate: 1.0 });
                }
            }
        }
        let sequential = parts.len() > 1;
        out.clips.push(Clip { name, parts, sequential });
    }
    out.clips.sort_by(|a, b| a.name.cmp(&b.name));
    // Animations no clip plays.
    out.animations.extend(stored.into_iter().zip(used).filter(|(_, used)| !used).map(|(a, _)| a));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rage_core::hash::joaat;

    const BASE: u64 = 0x5000_0000;

    fn put(m: &mut [u8], at: usize, v: &[u8]) {
        m[at..at + v.len()].copy_from_slice(v);
    }

    fn pointer(m: &mut [u8], at: usize, to: usize) {
        put(m, at, &(BASE + to as u64).to_le_bytes());
    }

    fn floats(m: &mut [u8], at: usize, v: &[f32]) {
        for (i, x) in v.iter().enumerate() {
            put(m, at + i * 4, &x.to_le_bytes());
        }
    }

    /// An animation of `frames` frames at `at`, its block and tracks behind it:
    /// the rotation of the bone `bone`: x static, y from 0 in steps of 0.1
    /// (4 bits), z static, w made from them.
    fn animation(m: &mut [u8], at: usize, bone: &str, frames: u16, duration: f32) {
        let (list, block, tracks) = (at + 0x80, at + 0x90, at + 0x140);
        put(m, at + 0x24, &frames.to_le_bytes());
        floats(m, at + 0x28, &[duration]);
        pointer(m, at + 0x50, list);
        put(m, at + 0x58, &[1]);
        pointer(m, at + 0x60, tracks);
        put(m, at + 0x68, &[1]);
        pointer(m, list, block);
        put(m, tracks, &joaat(bone).to_le_bytes());
        put(m, tracks + 4, &BONE_ROTATION.to_le_bytes());
        // Parameters: two static floats, a quantized one.
        let params = 4 + 4 + 12;
        let frame_bytes = usize::from(frames) * 4;
        let items = params + frame_bytes + 22;
        let len = items + 3 * 8;
        put(m, block + 4, &(len as u32).to_le_bytes());
        put(m, block + 12, &(params as u32).to_le_bytes());
        put(m, block + 0x16, &frames.to_le_bytes());
        put(m, block + 0x18, &[4]);
        put(m, block + 0x1C, &[4]);
        put(m, block + 0x1E, &[11]);
        let data = block + 0x20;
        floats(m, data, &[0.0, 0.0]);
        put(m, data + 8, &[4]);
        floats(m, data + 12, &[0.1, 0.0]);
        for f in 0..usize::from(frames) {
            put(m, data + params + f * 4, &[f as u8]);
        }
        for (kind, n) in [(2, 2u8), (4, 1), (7, 1)] {
            put(m, data + params + frame_bytes + kind * 2, &[n]);
        }
        // Items: track 0, components 0 and 2; component 1; w made.
        put(m, data + items, &[0, 0, 2]);
        put(m, data + items + 8, &[1]);
        put(m, data + items + 16, &[3]);
    }

    /// A dictionary with two animations and a clip of both as layers.
    fn dictionary() -> Resource {
        let mut m = vec![0u8; 0x800];
        pointer(&mut m, 0x38, 0x100);
        put(&mut m, 0x100, b"test@dictionary\0");
        pointer(&mut m, 0x48, 0x120); // clip buckets
        put(&mut m, 0x50, &[1]);
        pointer(&mut m, 0x70, 0x130); // animation dictionaries
        put(&mut m, 0x78, &[1]);
        pointer(&mut m, 0x120, 0x140); // bucket -> entry -> clip
        pointer(&mut m, 0x148, 0x180);
        pointer(&mut m, 0x130, 0x200);
        pointer(&mut m, 0x248, 0x260);
        put(&mut m, 0x250, &[1]);
        pointer(&mut m, 0x260, 0x270); // bucket -> two entries
        pointer(&mut m, 0x278, 0x400);
        pointer(&mut m, 0x280, 0x290);
        pointer(&mut m, 0x298, 0x600);
        put(&mut m, 0x180 + 0x20, &[2]);
        put(&mut m, 0x180 + 0x28, b"pack:/sit.clip\0");
        pointer(&mut m, 0x180 + 0x68, 0x2B0);
        put(&mut m, 0x180 + 0x70, &[1]);
        // The part: two layers, 0.2 s; the second runs at half the rate.
        pointer(&mut m, 0x2B0, 0x2D0);
        put(&mut m, 0x2B8, &[2]);
        floats(&mut m, 0x2C0, &[0.2]);
        pointer(&mut m, 0x2D0, 0x400);
        floats(&mut m, 0x2D0 + 0x28, &[0.2, 0.4, 1.0]);
        pointer(&mut m, 0x308, 0x600);
        floats(&mut m, 0x308 + 0x28, &[0.0, 0.2, 1.0]);
        animation(&mut m, 0x400, "SKEL_ROOT", 5, 0.4);
        animation(&mut m, 0x600, "SKEL_HEAD", 2, 0.2);
        Resource { version: 0, system: m, graphics: Vec::new() }
    }

    #[test]
    fn joins_the_layers_of_a_clip() {
        let dict = parse(&dictionary()).unwrap();
        let clip = &dict.clips[0];
        assert_eq!((clip.name.as_str(), clip.parts.len(), dict.animations.len()), ("sit", 1, 1));
        assert!((clip.duration() - 0.2).abs() < 1e-6);
        let a = &dict.animations[0];
        assert_eq!(a.frames, 3);
        let bones: Vec<u32> = a.tracks.iter().map(|t| t.bone_hash).collect();
        assert_eq!(bones, [joaat("skel_root"), joaat("skel_head")]);
        assert!(a.tracks.iter().all(|t| (t.track, t.format) == (TRACK_BONE_ROTATION, 1)));
        // The first layer starts at 0.2 s: frames 2 to 4; the second goes
        // from 0 to 0.1 in three frames instead of two.
        for (t, f, y) in [(0, 0, 0.2f32), (0, 1, 0.3), (0, 2, 0.4), (1, 0, 0.0), (1, 1, 0.05), (1, 2, 0.1)] {
            let q = a.value(t, f as f32).unwrap();
            assert!((q[1] - y).abs() < 1e-3, "track {t} frame {f}: {q:?}");
            assert!((q.iter().map(|c| c * c).sum::<f32>() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn names_by_pointer_or_in_place() {
        let mut res = dictionary();
        pointer(&mut res.system, 0x180 + 0x28, 0x340);
        put(&mut res.system, 0x340, b"pack:/a_name_too_long_to_fit_in.clip\0");
        assert_eq!(parse(&res).unwrap().clips[0].name, "a_name_too_long_to_fit_in");
    }

    #[test]
    fn refuses_what_does_not_add_up() {
        let broken = |at: usize, v: &[u8]| {
            let mut res = dictionary();
            put(&mut res.system, at, v);
            parse(&res).is_err()
        };
        // A clip of another kind, a block of other channel kinds, another
        // frame length, frames that do not add up, a track without channels.
        assert!(broken(0x1A0, &[1]));
        assert!(broken(0x400 + 0x90 + 0x1E, &[10]));
        assert!(broken(0x400 + 0x90 + 0x18, &[8]));
        assert!(broken(0x400 + 0x24, &[6]));
        assert!(broken(0x400 + 0x68, &[2]));
        // A track of an unknown format, a layer of an animation not there.
        assert!(broken(0x400 + 0x140 + 8, &[7]));
        assert!(broken(0x2D0, &[0x10]));
        assert!(parse(&Resource { version: 0, system: vec![0; 0x100], graphics: Vec::new() }).is_err());
    }
}
