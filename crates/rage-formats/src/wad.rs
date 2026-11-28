//! GTA IV animation dictionaries (`.wad`), read into the
//! [`ClipDictionary`] the rest of OpenRAGE plays: one clip per animation.
//!
//! * Dictionary root: `0x10` name hashes, `0x18` animations (pointer, u16 count).
//! * Animation: `0x08` frame count (u16), `0x0C` duration (seconds), `0x1C`
//!   name (`pack:/<name>.anim`), `0x20` tracks (pointer list, count at `0x24`).
//! * Track (16 bytes): kind (u8: 0 bone translation, 1 bone rotation, 5 and 6
//!   mover translation and rotation, 7 and 8 camera translation and rotation,
//!   128 and up other values, 136 the camera's field of view), bone tag (u16
//!   at `0x02`), frames per block minus one (`0x04`), channel groups
//!   (pointer list at `0x08`, one per block, count u16 at `0x0C`) → group:
//!   channels (up to four pointers from `0x04`), channel count `0x14`.
//!   Animations longer than a block (240 frames, 8 seconds) have several;
//!   each block starts on the last frame of the one before.
//! * Channel: type at `0x05`:
//!   1 raw floats (data `0x08`, count `0x0C`),
//!   4 a constant float (`0x08`),
//!   6 quantised floats (data `0x08`, bits per value `0x0C`, count `0x10`,
//!   scale `0x14`, offset `0x18`; values packed LSB first),
//!   9 a constant quaternion (pointer `0x08`),
//!   13 a constant vector (pointer `0x08`).
//!   Types 7, 11 and 12 only occur in the non-bone tracks, which are skipped.
//!
//! A vector track has three float channels or one vector channel, a
//! quaternion track four float channels or one quaternion channel. The field
//! of view (degrees) is one float channel, kept as a vector `(fov, 0, 0)`
//! under [`TRACK_CAMERA_FOV`], the track GTA V's cutscene cameras use.
//!
//! Cutscene lights move their root (kind 0) and have tracks of their own,
//! kept under their kind: colour (139) and direction (143) as vectors,
//! reach (140), intensity (141) and cone angle (142) as floats.

use rage_resource::{Resource, ResourceError};

use crate::clip::{Animation, BoneTrack, Clip, ClipDictionary, ClipPart};

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

fn ptr(res: &Resource, at: u64) -> Result<u64, ResourceError> {
    res.u32(at).map(u64::from)
}

fn f32_at(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    Ok(f32::from_bits(res.u32(at)?))
}

/// Track kinds that move bones (the others are skipped).
const KIND_TRANSLATION: u8 = 0;
const KIND_ROTATION: u8 = 1;
const KIND_MOVER_TRANSLATION: u8 = 5;
const KIND_MOVER_ROTATION: u8 = 6;
const KIND_CAMERA_TRANSLATION: u8 = 7;
const KIND_CAMERA_ROTATION: u8 = 8;
const KIND_CAMERA_FOV: u8 = 136;

/// A cutscene light's colour (vector).
pub const TRACK_LIGHT_COLOUR: u8 = 139;
/// A cutscene light's reach in metres (float).
pub const TRACK_LIGHT_RANGE: u8 = 140;
/// A cutscene light's intensity (float; 0 is off).
pub const TRACK_LIGHT_INTENSITY: u8 = 141;
/// A cutscene light's cone angle in degrees (float).
pub const TRACK_LIGHT_CONE: u8 = 142;
/// A cutscene light's direction (vector).
pub const TRACK_LIGHT_DIRECTION: u8 = 143;

/// Kinds with one float channel, kept as a vector `(value, 0, 0)`.
fn is_float_kind(kind: u8) -> bool {
    matches!(kind, KIND_CAMERA_FOV | TRACK_LIGHT_RANGE | TRACK_LIGHT_INTENSITY | TRACK_LIGHT_CONE)
}

/// The track a camera's field of view is kept under (GTA V's number for it).
pub const TRACK_CAMERA_FOV: u8 = 27;

/// Channel types of a raw, a constant and a quantised float.
const CHANNEL_RAW: u8 = 1;
const CHANNEL_CONSTANT_FLOAT: u8 = 4;
const CHANNEL_QUANTISED: u8 = 6;

/// One float channel, decoded for every frame.
fn float_channel(res: &Resource, ch: u64, frames: usize) -> Result<Vec<f32>, ResourceError> {
    match res.u8(ch + 0x05)? {
        CHANNEL_RAW => raw_channel(res, ch, frames),
        CHANNEL_CONSTANT_FLOAT => Ok(vec![f32_at(res, ch + 0x08)?; frames]),
        CHANNEL_QUANTISED => quantised_channel(res, ch, frames),
        t => Err(bad(format!("channel type {t} where a float was expected"))),
    }
}

fn raw_channel(res: &Resource, ch: u64, frames: usize) -> Result<Vec<f32>, ResourceError> {
    let data = ptr(res, ch + 0x08)?;
    let count = res.u16(ch + 0x0C)? as usize;
    if count == 0 || count > 1 << 16 {
        return Err(bad(format!("raw channel with {count} values")));
    }
    let raw = res.bytes(data, count * 4)?;
    let values: Vec<f32> = raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    Ok((0..frames).map(|f| values[f.min(count - 1)]).collect())
}

/// Where a quantised channel's values are, how wide they are and how they map to floats.
struct Quantised {
    data: u64,
    bits: u32,
    count: usize,
    scale: f32,
    offset: f32,
}

fn read_quantised(res: &Resource, ch: u64) -> Result<Quantised, ResourceError> {
    let data = ptr(res, ch + 0x08)?;
    let bits = res.u32(ch + 0x0C)?;
    let count = res.u32(ch + 0x10)? as usize;
    let (scale, offset) = (f32_at(res, ch + 0x14)?, f32_at(res, ch + 0x18)?);
    if bits == 0 || bits > 32 || count == 0 || count > 1 << 16 {
        return Err(bad(format!("quantised channel: {bits} bits, {count} values")));
    }
    Ok(Quantised { data, bits, count, scale, offset })
}

fn quantised_channel(res: &Resource, ch: u64, frames: usize) -> Result<Vec<f32>, ResourceError> {
    let Quantised { data, bits, count, scale, offset } = read_quantised(res, ch)?;
    let words = (bits as usize * count).div_ceil(32);
    let raw = res.bytes(data, words * 4)?;
    Ok((0..frames).map(|f| offset + quantised_value(raw, bits, f.min(count - 1)) as f32 * scale).collect())
}

/// The `i`th of `bits`-wide values packed little-endian into `raw`.
fn quantised_value(raw: &[u8], bits: u32, i: usize) -> u64 {
    let start = i * bits as usize;
    let mut v = 0u64;
    for k in 0..((start % 8 + bits as usize).div_ceil(8)) {
        v |= (raw.get(start / 8 + k).copied().unwrap_or(0) as u64) << (k * 8);
    }
    (v >> (start % 8)) & ((1u64 << bits) - 1)
}

/// A bone track with its value for every frame.
type Track = (BoneTrack, Vec<[f32; 4]>);

/// Whether a track of this kind holds quaternions (rather than vectors); `None` for kinds that are skipped.
fn is_rotation_kind(kind: u8) -> Option<bool> {
    match kind {
        KIND_TRANSLATION
        | KIND_MOVER_TRANSLATION
        | KIND_CAMERA_TRANSLATION
        | TRACK_LIGHT_COLOUR
        | TRACK_LIGHT_DIRECTION => Some(false),
        k if is_float_kind(k) => Some(false),
        KIND_ROTATION | KIND_MOVER_ROTATION | KIND_CAMERA_ROTATION => Some(true),
        _ => None,
    }
}

/// Values of one track for every frame, or `None` for tracks that are skipped.
fn track_values(res: &Resource, track: u64, frames: usize) -> Result<Option<Track>, ResourceError> {
    let kind = res.u8(track)?;
    let Some(quat) = is_rotation_kind(kind) else { return Ok(None) };
    let bone = res.u16(track + 0x02)?;
    let layout = read_layout(res, track)?;
    let Some(values) = layout.values(res, kind, quat, frames)? else { return Ok(None) };
    let track = match kind {
        KIND_CAMERA_FOV => BoneTrack { bone, format: 0, track: TRACK_CAMERA_FOV, ..Default::default() },
        _ => BoneTrack { bone, format: quat as u8, track: kind, ..Default::default() },
    };
    Ok(Some((track, values)))
}

/// How a track's frames are split into blocks.
struct Layout {
    /// Blocks of `stride + 1` frames, each starting on the last frame of the one before.
    stride: usize,
    blocks: usize,
    slots: u64,
}

fn read_layout(res: &Resource, track: u64) -> Result<Layout, ResourceError> {
    let stride = res.u32(track + 0x04)? as usize;
    let blocks = (res.u16(track + 0x0C)? as usize).max(1);
    let slots = ptr(res, track + 0x08)?;
    if stride == 0 && blocks > 1 || blocks > 1 << 12 {
        return Err(bad(format!("track in {blocks} blocks of {stride} frames")));
    }
    Ok(Layout { stride, blocks, slots })
}

impl Layout {
    /// The values of every block joined, padded to `frames`; `None` for layouts that are skipped.
    fn values(
        &self,
        res: &Resource,
        kind: u8,
        quat: bool,
        frames: usize,
    ) -> Result<Option<Vec<[f32; 4]>>, ResourceError> {
        let mut values: Vec<[f32; 4]> = Vec::with_capacity(frames);
        for b in 0..self.blocks {
            let start = b * self.stride;
            if start >= frames {
                break;
            }
            let len = if self.blocks == 1 { frames } else { (self.stride + 1).min(frames - start) };
            let group = ptr(res, self.slots + b as u64 * 4)?;
            let Some(block) = block_values(res, group, kind, quat, len)? else { return Ok(None) };
            // The first frame repeats the previous block's last.
            values.extend(block.into_iter().skip(usize::from(b > 0)));
        }
        let Some(&last) = values.last() else { return Ok(None) };
        values.resize(frames, last);
        Ok(Some(values))
    }
}

/// Channel types of a constant quaternion and a constant vector.
const CHANNEL_CONSTANT_QUATERNION: u8 = 9;
const CHANNEL_CONSTANT_VECTOR: u8 = 13;

/// A constant quaternion channel, for every frame.
fn constant_quaternion(res: &Resource, ch: u64, frames: usize) -> Result<Vec<[f32; 4]>, ResourceError> {
    let at = ptr(res, ch + 0x08)?;
    let q = [f32_at(res, at)?, f32_at(res, at + 4)?, f32_at(res, at + 8)?, f32_at(res, at + 12)?];
    Ok(vec![q; frames])
}

/// A constant vector channel, for every frame.
fn constant_vector(res: &Resource, ch: u64, frames: usize) -> Result<Vec<[f32; 4]>, ResourceError> {
    let at = ptr(res, ch + 0x08)?;
    Ok(vec![[f32_at(res, at)?, f32_at(res, at + 4)?, f32_at(res, at + 8)?, 0.0]; frames])
}

/// A single channel holding a constant value of the wanted shape, for every frame.
fn constant_block(res: &Resource, ch: u64, quat: bool, frames: usize) -> Result<Option<Vec<[f32; 4]>>, ResourceError> {
    let kind = res.u8(ch + 0x05)?;
    if quat && kind == CHANNEL_CONSTANT_QUATERNION {
        return constant_quaternion(res, ch, frames).map(Some);
    }
    if !quat && kind == CHANNEL_CONSTANT_VECTOR {
        return constant_vector(res, ch, frames).map(Some);
    }
    Ok(None)
}

/// Four float channels as normalised quaternions.
fn quaternion_block(res: &Resource, channels: [u64; 4], frames: usize) -> Result<Vec<[f32; 4]>, ResourceError> {
    let [x, y, z, w] = channels.map(|ch| float_channel(res, ch, frames));
    let (x, y, z, w) = (x?, y?, z?, w?);
    Ok((0..frames)
        .map(|f| {
            let q = [x[f], y[f], z[f], w[f]];
            let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            if len > 1e-6 { q.map(|v| v / len) } else { [0.0, 0.0, 0.0, 1.0] }
        })
        .collect())
}

/// Three float channels as vectors.
fn vector_block(res: &Resource, [x, y, z]: [u64; 3], frames: usize) -> Result<Vec<[f32; 4]>, ResourceError> {
    let (x, y, z) = (float_channel(res, x, frames)?, float_channel(res, y, frames)?, float_channel(res, z, frames)?);
    Ok((0..frames).map(|f| [x[f], y[f], z[f], 0.0]).collect())
}

/// A float track's block: its single channel as vectors `(value, 0, 0)`.
fn float_block(res: &Resource, channels: &[u64], frames: usize) -> Result<Option<Vec<[f32; 4]>>, ResourceError> {
    let [ch] = channels else { return Ok(None) };
    Ok(Some(float_channel(res, *ch, frames)?.into_iter().map(|v| [v, 0.0, 0.0, 0.0]).collect()))
}

/// One block of a track: `frames` values from its channel group, or `None`
/// for layouts that are skipped.
fn block_values(
    res: &Resource,
    group: u64,
    kind: u8,
    quat: bool,
    frames: usize,
) -> Result<Option<Vec<[f32; 4]>>, ResourceError> {
    let count = res.u32(group + 0x14)? as u64;
    let channels: Vec<u64> = (0..count.min(4)).map(|k| ptr(res, group + 0x04 + k * 4)).collect::<Result<_, _>>()?;
    if is_float_kind(kind) {
        return float_block(res, &channels, frames);
    }
    match (quat, channels.as_slice()) {
        (_, [ch]) => constant_block(res, *ch, quat, frames),
        (true, [x, y, z, w]) => Ok(Some(quaternion_block(res, [*x, *y, *z, *w], frames)?)),
        (false, [x, y, z]) => Ok(Some(vector_block(res, [*x, *y, *z], frames)?)),
        // A handful of rotations in Euler form and other rare layouts.
        _ => Ok(None),
    }
}

fn read_animation(res: &Resource, a: u64) -> Result<(String, Animation), ResourceError> {
    let frames = res.u16(a + 0x08)? as usize;
    let duration = f32_at(res, a + 0x0C)?;
    if frames == 0 || frames > 1 << 16 {
        return Err(bad(format!("animation with {frames} frames")));
    }
    let name_ptr = ptr(res, a + 0x1C)?;
    let name = if name_ptr != 0 { res.string(name_ptr).unwrap_or_default() } else { String::new() };
    let name = name.strip_prefix("pack:/").unwrap_or(&name);
    let name = name.strip_suffix(".anim").unwrap_or(name).to_owned();
    let list = ptr(res, a + 0x20)?;
    let count = res.u16(a + 0x24)? as u64;
    let mut tracks = Vec::new();
    let mut values = Vec::new();
    for i in 0..count.min(4096) {
        let t = ptr(res, list + i * 4)?;
        if let Some((track, v)) = track_values(res, t, frames)? {
            tracks.push(track);
            values.push(v);
        }
    }
    Ok((name, Animation::from_frames(frames, duration, tracks, values)))
}

/// Reads a `.wad` file: one clip per animation, named after it.
pub fn parse_wad(res: &Resource) -> Result<ClipDictionary, ResourceError> {
    let root = Resource::ROOT;
    let list = ptr(res, root + 0x18)?;
    let count = res.u16(root + 0x1C)? as u64;
    let mut dict = ClipDictionary::default();
    for i in 0..count {
        let a = ptr(res, list + i * 4)?;
        if a == 0 {
            continue;
        }
        let (name, animation) = read_animation(res, a)?;
        let name = if name.is_empty() { format!("anim_{i}") } else { name };
        let end = animation.duration;
        dict.clips.push(Clip {
            name,
            parts: vec![ClipPart { animation: dict.animations.len(), start: 0.0, end, rate: 1.0 }],
            sequential: false,
        });
        dict.animations.push(animation);
    }
    Ok(dict)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);

    impl Mem {
        fn put(&mut self, at: usize, v: u32) {
            self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        fn putf(&mut self, at: usize, v: f32) {
            self.put(at, v.to_bits());
        }
    }

    const S: u32 = 0x5000_0000;

    /// One animation of 3 frames: a rotation of bone 0x1A1 from three
    /// constant channels and a quantised one, and a raw root translation.
    fn wad() -> Resource {
        let mut m = Mem(vec![0; 0x400]);
        m.put(0x18, S + 0x40);
        m.0[0x1C] = 1;
        m.put(0x40, S + 0x50);
        let a = 0x50;
        m.0[a + 0x08] = 3;
        m.putf(a + 0x0C, 0.1);
        m.put(a + 0x1C, S + 0x3E0);
        m.0[0x3E0..0x3E0 + 13].copy_from_slice(b"pack:/a.anim\0");
        m.put(a + 0x20, S + 0x80);
        m.0[a + 0x24] = 2;
        m.put(0x80, S + 0x90);
        m.put(0x84, S + 0xA0);
        // track 0: rotation of bone 0x1A1 → slot → group
        m.0[0x90] = 1;
        m.0[0x92..0x94].copy_from_slice(&0x1A1u16.to_le_bytes());
        m.put(0x98, S + 0xB0);
        m.put(0xB0, S + 0xC0);
        let g = 0xC0;
        for (k, ch) in [0x100u32, 0x110, 0x120, 0x130].iter().enumerate() {
            m.put(g + 4 + k * 4, S + ch);
        }
        m.put(g + 0x14, 4);
        for (ch, v) in [(0x100, 0.0f32), (0x110, 0.0), (0x120, 0.0)] {
            m.0[ch + 5] = 4;
            m.putf(ch + 8, v);
        }
        // w: quantised, 2 bits per value: 1, 2, 3 → 0.5, 1.0, 1.5 (normalised to 1)
        let w = 0x130;
        m.0[w + 5] = 6;
        m.put(w + 0x08, S + 0x160);
        m.put(w + 0x0C, 2);
        m.put(w + 0x10, 3);
        m.putf(w + 0x14, 0.5);
        m.putf(w + 0x18, 0.0);
        m.0[0x160] = 0b11_10_01;
        // track 1: root translation, three raw channels
        m.put(0xA8, S + 0x200);
        m.put(0x200, S + 0x210);
        let g = 0x210;
        for (k, ch) in [0x240u32, 0x260, 0x280].iter().enumerate() {
            m.put(g + 4 + k * 4, S + ch);
            m.0[*ch as usize + 5] = 1;
            m.put(*ch as usize + 8, S + ch + 0x10);
            m.0[*ch as usize + 0x0C] = 3;
            for f in 0..3 {
                m.putf(*ch as usize + 0x10 + f * 4, (k * 10 + f) as f32);
            }
        }
        m.put(g + 0x14, 3);
        Resource { version: 1, system: m.0, graphics: Vec::new() }
    }

    #[test]
    fn decodes_tracks() {
        let dict = parse_wad(&wad()).unwrap();
        assert_eq!(dict.clips[0].name, "a");
        let a = &dict.animations[0];
        assert_eq!((a.frames, a.tracks.len()), (3, 2));
        assert_eq!((a.tracks[0].bone, a.tracks[0].format, a.tracks[0].track), (0x1A1, 1, 1));
        assert_eq!(a.value(0, 2.0), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(a.value(1, 1.0), Some([1.0, 11.0, 21.0, 0.0]));
    }

    #[test]
    fn keeps_the_camera_field_of_view() {
        let mut res = wad();
        // Track 1 becomes a field of view: one channel (0, 1, 2 degrees).
        res.system[0xA0] = 136;
        res.system[0x210 + 0x14] = 1;
        let a = &parse_wad(&res).unwrap().animations[0];
        assert_eq!((a.tracks[1].track, a.tracks[1].format), (TRACK_CAMERA_FOV, 0));
        assert_eq!(a.value(1, 2.0), Some([2.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn joins_blocks() {
        let mut res = wad();
        let put = |res: &mut Resource, at: usize, v: u32| res.system[at..at + 4].copy_from_slice(&v.to_le_bytes());
        // Track 1 in two blocks of two frames (stride 1): the second block starts on frame 1.
        put(&mut res, 0xA4, 1);
        res.system[0xAC] = 2;
        put(&mut res, 0x204, S + 0x2A0);
        put(&mut res, 0x2A0 + 0x14, 3);
        for (k, ch) in [0x2C0usize, 0x2E0, 0x300].into_iter().enumerate() {
            put(&mut res, 0x2A4 + k * 4, S + ch as u32);
            res.system[ch + 5] = 1;
            put(&mut res, ch + 8, S + ch as u32 + 0x10);
            res.system[ch + 0x0C] = 2;
            for f in 0..2 {
                put(&mut res, ch + 0x10 + f * 4, ((k * 10 + 5 + f) as f32).to_bits());
            }
        }
        let a = &parse_wad(&res).unwrap().animations[0];
        assert_eq!(a.value(1, 0.0), Some([0.0, 10.0, 20.0, 0.0]));
        assert_eq!(a.value(1, 1.0), Some([1.0, 11.0, 21.0, 0.0]));
        assert_eq!(a.value(1, 2.0), Some([6.0, 16.0, 26.0, 0.0]), "the second block's second frame");
    }
}
