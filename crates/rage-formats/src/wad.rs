//! GTA IV animation dictionaries (`.wad`), read into the
//! [`ClipDictionary`] the rest of OpenRAGE plays: one clip per animation.
//!
//! * Dictionary root: `0x10` name hashes, `0x18` animations (pointer, u16 count).
//! * Animation: `0x08` frame count (u16), `0x0C` duration (seconds), `0x1C`
//!   name (`pack:/<name>.anim`), `0x20` tracks (pointer list, count at `0x24`).
//! * Track (16 bytes): kind (u8: 0 bone translation, 1 bone rotation, 5 and 6
//!   mover translation and rotation, 128 and up other values), bone tag (u16
//!   at `0x02`), channel group (pointer at `0x08`) → group: channels (up to
//!   four pointers from `0x04`), channel count `0x14`.
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
//! quaternion track four float channels or one quaternion channel.

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

/// One float channel, decoded for every frame.
fn float_channel(res: &Resource, ch: u64, frames: usize) -> Result<Vec<f32>, ResourceError> {
    match res.u8(ch + 0x05)? {
        1 => {
            let data = ptr(res, ch + 0x08)?;
            let count = res.u16(ch + 0x0C)? as usize;
            if count == 0 || count > 1 << 16 {
                return Err(bad(format!("raw channel with {count} values")));
            }
            let raw = res.bytes(data, count * 4)?;
            let values: Vec<f32> = raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
            Ok((0..frames).map(|f| values[f.min(count - 1)]).collect())
        }
        4 => Ok(vec![f32_at(res, ch + 0x08)?; frames]),
        6 => {
            let data = ptr(res, ch + 0x08)?;
            let bits = res.u32(ch + 0x0C)?;
            let count = res.u32(ch + 0x10)? as usize;
            let (scale, offset) = (f32_at(res, ch + 0x14)?, f32_at(res, ch + 0x18)?);
            if bits == 0 || bits > 32 || count == 0 || count > 1 << 16 {
                return Err(bad(format!("quantised channel: {bits} bits, {count} values")));
            }
            let words = (bits as usize * count).div_ceil(32);
            let raw = res.bytes(data, words * 4)?;
            let value = |i: usize| -> f32 {
                let start = i * bits as usize;
                let mut v = 0u64;
                for k in 0..((start % 8 + bits as usize).div_ceil(8)) {
                    v |= (raw.get(start / 8 + k).copied().unwrap_or(0) as u64) << (k * 8);
                }
                let q = (v >> (start % 8)) & ((1u64 << bits) - 1);
                offset + q as f32 * scale
            };
            Ok((0..frames).map(|f| value(f.min(count - 1))).collect())
        }
        t => Err(bad(format!("channel type {t} where a float was expected"))),
    }
}

/// A bone track with its value for every frame.
type Track = (BoneTrack, Vec<[f32; 4]>);

/// Values of one track for every frame, or `None` for tracks that are skipped.
fn track_values(res: &Resource, track: u64, frames: usize) -> Result<Option<Track>, ResourceError> {
    let kind = res.u8(track)?;
    let quat = match kind {
        KIND_TRANSLATION | KIND_MOVER_TRANSLATION => false,
        KIND_ROTATION | KIND_MOVER_ROTATION => true,
        _ => return Ok(None),
    };
    let bone = res.u16(track + 0x02)?;
    let group = ptr(res, ptr(res, track + 0x08)?)?;
    let count = res.u32(group + 0x14)? as u64;
    let channels: Vec<u64> = (0..count.min(4)).map(|k| ptr(res, group + 0x04 + k * 4)).collect::<Result<_, _>>()?;
    let values: Vec<[f32; 4]> = match (quat, channels.as_slice()) {
        (true, [ch]) if res.u8(ch + 0x05)? == 9 => {
            let at = ptr(res, ch + 0x08)?;
            let q = [f32_at(res, at)?, f32_at(res, at + 4)?, f32_at(res, at + 8)?, f32_at(res, at + 12)?];
            vec![q; frames]
        }
        (false, [ch]) if res.u8(ch + 0x05)? == 13 => {
            let at = ptr(res, ch + 0x08)?;
            vec![[f32_at(res, at)?, f32_at(res, at + 4)?, f32_at(res, at + 8)?, 0.0]; frames]
        }
        (true, [x, y, z, w]) => {
            let c = [*x, *y, *z, *w].map(|ch| float_channel(res, ch, frames));
            let [x, y, z, w] = c;
            let (x, y, z, w) = (x?, y?, z?, w?);
            (0..frames)
                .map(|f| {
                    let q = [x[f], y[f], z[f], w[f]];
                    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
                    if len > 1e-6 { q.map(|v| v / len) } else { [0.0, 0.0, 0.0, 1.0] }
                })
                .collect()
        }
        (false, [x, y, z]) => {
            let (x, y, z) =
                (float_channel(res, *x, frames)?, float_channel(res, *y, frames)?, float_channel(res, *z, frames)?);
            (0..frames).map(|f| [x[f], y[f], z[f], 0.0]).collect()
        }
        // A handful of rotations in Euler form and other rare layouts.
        _ => return Ok(None),
    };
    Ok(Some((BoneTrack { bone, format: quat as u8, track: kind }, values)))
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
}
