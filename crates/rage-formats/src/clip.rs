//! Clip dictionaries (`.ycd`): animations of skeletons.
//!
//! Layout (format knowledge from CodeWalker, MIT):
//! * dictionary: `0x18` animation map, `0x28` clip hash buckets (`0x30`
//!   bucket count); bucket entries chain through `0x10`, clip at `0x08`;
//! * clip: type byte at `0x10` (1 = one animation, 2 = list), name at `0x18`;
//!   type 1: animation `0x50`, start/end/rate at `0x58`; type 2: list `0x50`,
//!   count `0x58`, entries of 0x18 bytes (start, end, rate, …, animation);
//! * animation: frames `0x14`, frames per sequence block `0x16`, duration
//!   `0x18`, sequence blocks `0x40` (pointer list), bone tracks `0x50`
//!   (u16 bone tag, u8 format, u8 track);
//! * sequence block: 32-byte header (data length, frame data offset, frame
//!   count, frame length, …) then the packed channel data. Every bone track
//!   is an "anim sequence" of up to four channels; channel parameters come
//!   first, then per-frame bit fields, then the channel list.

use rage_resource::{Resource, ResourceError};

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

pub const TRACK_BONE_TRANSLATION: u8 = 0;
pub const TRACK_BONE_ROTATION: u8 = 1;
pub const TRACK_BONE_SCALE: u8 = 2;

#[derive(Debug, Clone, Copy)]
pub struct BoneTrack {
    /// Bone tag (`0` is the skeleton root).
    pub bone: u16,
    /// 0: vector, 1: quaternion.
    pub format: u8,
    /// What is animated: 0 translation, 1 rotation, 2 scale, 5/6 mover, …
    pub track: u8,
}

/// One block of frames: decoded values per bone track and frame.
#[derive(Debug, Clone)]
struct Block {
    frames: usize,
    /// `values[track][frame]`: vector (x, y, z, 0) or quaternion (x, y, z, w).
    values: Vec<Vec<[f32; 4]>>,
}

#[derive(Debug, Clone)]
pub struct Animation {
    pub frames: usize,
    pub frames_per_block: usize,
    pub duration: f32,
    pub tracks: Vec<BoneTrack>,
    blocks: Vec<Block>,
}

#[derive(Debug, Clone)]
pub struct ClipPart {
    pub animation: usize,
    pub start: f32,
    pub end: f32,
    pub rate: f32,
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub name: String,
    pub parts: Vec<ClipPart>,
}

impl Clip {
    /// Length in seconds (the longest part).
    pub fn duration(&self) -> f32 {
        self.parts.iter().map(|p| (p.end - p.start).max(0.0) / p.rate.max(0.001)).fold(0.0, f32::max)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClipDictionary {
    pub clips: Vec<Clip>,
    pub animations: Vec<Animation>,
}

// ---------------------------------------------------------------------------
// Bit reading

/// LSB-first bit extraction; bytes past the end read as zero.
fn bits(data: &[u8], start: usize, len: u32) -> u32 {
    let mut value = 0u64;
    let first = start / 8;
    let shift = start % 8;
    let needed = (shift + len as usize).div_ceil(8);
    for i in 0..needed.min(8) {
        value |= (data.get(first + i).copied().unwrap_or(0) as u64) << (i * 8);
    }
    ((value >> shift) & ((1u64 << len) - 1)) as u32
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> Result<u32, ResourceError> {
        let v = self.data.get(self.pos..self.pos + 4).ok_or_else(|| bad("channel parameters truncated"))?;
        self.pos += 4;
        Ok(u32::from_le_bytes(v.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, ResourceError> {
        self.u32().map(f32::from_bits)
    }
}

// ---------------------------------------------------------------------------
// Channels

#[derive(Debug, Clone)]
enum Channel {
    StaticQuaternion([f32; 4]),
    StaticVector3([f32; 3]),
    StaticFloat(f32),
    RawFloat {
        bit: usize,
    },
    QuantizeFloat {
        bit: usize,
        bits: u32,
        quantum: f32,
        offset: f32,
    },
    IndirectQuantizeFloat {
        bit: usize,
        frame_bits: u32,
        table: Vec<f32>,
    },
    LinearFloat(Vec<f32>),
    /// The derived component of a quaternion stored as three floats; `index`
    /// is where it goes (0 = x … 3 = w).
    CachedQuaternion {
        index: usize,
    },
}

impl Channel {
    fn frame_bits(&self) -> u32 {
        match self {
            Channel::RawFloat { .. } => 32,
            Channel::QuantizeFloat { bits, .. } => *bits,
            Channel::IndirectQuantizeFloat { frame_bits, .. } => *frame_bits,
            _ => 0,
        }
    }

    fn set_bit(&mut self, at: usize) {
        match self {
            Channel::RawFloat { bit }
            | Channel::QuantizeFloat { bit, .. }
            | Channel::IndirectQuantizeFloat { bit, .. } => *bit = at,
            _ => {}
        }
    }

    /// Scalar value of a float channel at `frame` (`frame_start` is the bit
    /// position of the frame's packed data).
    fn float(&self, data: &[u8], frame: usize, frame_start: usize) -> f32 {
        match self {
            Channel::StaticFloat(v) => *v,
            Channel::RawFloat { bit } => f32::from_bits(bits(data, frame_start + bit, 32)),
            Channel::QuantizeFloat { bit, bits: n, quantum, offset } => {
                bits(data, frame_start + bit, *n) as f32 * quantum + offset
            }
            Channel::IndirectQuantizeFloat { bit, frame_bits, table } => {
                let i = bits(data, frame_start + bit, *frame_bits) as usize;
                table.get(i).copied().unwrap_or(0.0)
            }
            Channel::LinearFloat(values) => values.get(frame).or(values.last()).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    }
}

/// Reads a channel's parameters (and, for table-based channels, its table).
fn read_channel(kind: u16, c: &mut Cursor, frames: usize) -> Result<Channel, ResourceError> {
    Ok(match kind {
        0 => {
            let v = [c.f32()?, c.f32()?, c.f32()?];
            let w = (1.0 - (v[0] * v[0] + v[1] * v[1] + v[2] * v[2])).max(0.0).sqrt();
            Channel::StaticQuaternion([v[0], v[1], v[2], w])
        }
        1 => Channel::StaticVector3([c.f32()?, c.f32()?, c.f32()?]),
        2 => Channel::StaticFloat(c.f32()?),
        3 => Channel::RawFloat { bit: 0 },
        4 => {
            let bits = c.u32()?;
            Channel::QuantizeFloat { bit: 0, bits: bits.min(32), quantum: c.f32()?, offset: c.f32()? }
        }
        5 => {
            let frame_bits = c.u32()?.min(32);
            let value_bits = c.u32()?.clamp(1, 32);
            let num_ints = c.u32()? as usize;
            let quantum = c.f32()?;
            let offset = c.f32()?;
            let start = c.pos * 8;
            let count = ((num_ints * 32) / value_bits as usize).min((1usize << frame_bits.min(20)).saturating_sub(1));
            let table = (0..count)
                .map(|i| bits(c.data, start + i * value_bits as usize, value_bits) as f32 * quantum + offset)
                .collect();
            c.pos += num_ints * 4;
            Channel::IndirectQuantizeFloat { bit: 0, frame_bits, table }
        }
        6 => {
            // `num_ints` covers the whole channel, this 16-byte header included.
            let start = c.pos;
            let num_ints = c.u32()? as usize;
            let counts = c.u32()?;
            let quantum = c.f32()?;
            let offset = c.f32()?;
            let values = linear_float(c.data, c.pos * 8, frames, counts, quantum, offset);
            c.pos = start + (num_ints * 4).max(16);
            Channel::LinearFloat(values)
        }
        7 | 8 => Channel::CachedQuaternion { index: 0 },
        k => return Err(bad(format!("unknown animation channel type {k}"))),
    })
}

/// Delta-coded values in chunks of 64 frames: per chunk a bit offset and a
/// start value, then per frame a delta of the increment (`count3` low bits,
/// unary high bits, sign bit).
fn linear_float(data: &[u8], base: usize, frames: usize, counts: u32, quantum: f32, offset: f32) -> Vec<f32> {
    const CHUNK: usize = 64;
    let (c1, c2, c3) = (counts & 0xFF, (counts >> 8) & 0xFF, (counts >> 16) & 0xFF);
    let chunks = frames.div_ceil(CHUNK).max(1);
    let deltas = base + chunks * (c1 + c2) as usize;
    let mut out = Vec::with_capacity(frames);
    for chunk in 0..chunks {
        let offset_bits = bits(data, base + chunk * c1 as usize, c1.min(32)) as usize;
        let start = bits(data, base + chunks * c1 as usize + chunk * c2 as usize, c2.min(32)) as i64;
        let mut pos = deltas + offset_bits;
        let mut value = start;
        let mut inc: i64 = 0;
        for k in 0..CHUNK.min(frames - chunk * CHUNK) {
            if k > 0 {
                let mut delta = bits(data, pos, c3.min(32)) as i64;
                pos += c3 as usize;
                let mut zeros = 0;
                while bits(data, pos, 1) == 0 && zeros < 32 {
                    zeros += 1;
                    pos += 1;
                }
                pos += 1;
                delta |= (zeros as i64) << c3;
                if delta != 0 {
                    if bits(data, pos, 1) == 1 {
                        delta = -delta;
                    }
                    pos += 1;
                }
                inc += delta;
                value += inc;
            }
            out.push(value as f32 * quantum + offset);
        }
    }
    out
}

/// Decodes one sequence block (header at `ptr`).
fn read_block(res: &Resource, ptr: u64, tracks: usize) -> Result<Block, ResourceError> {
    let data_len = res.u32(ptr + 0x04)? as usize;
    let frame_offset = res.u32(ptr + 0x0C)? as usize;
    let frames = res.u16(ptr + 0x16)? as usize;
    let frame_len = res.u16(ptr + 0x18)? as usize;
    let data = res.bytes(ptr + 0x20, data_len)?;
    let list = frame_offset + frame_len * frames;
    let count = |t: usize| -> Result<usize, ResourceError> {
        data.get(list + t * 2..list + t * 2 + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(|| bad("channel list outside the block"))
    };

    // (anim sequence, slot, channel)
    let mut channels: Vec<(usize, usize, Channel)> = Vec::new();
    let mut params = Cursor { data, pos: 0 };
    let mut item = list + 18;
    let mut frame_bit = 0usize;
    for kind in 0..9u16 {
        let n = count(kind as usize)?;
        for _ in 0..n {
            let raw = data.get(item..item + 2).ok_or_else(|| bad("channel item outside the block"))?;
            let raw = u16::from_le_bytes([raw[0], raw[1]]) as usize;
            item += 2;
            let mut ch = read_channel(kind, &mut params, frames)?;
            ch.set_bit(frame_bit);
            frame_bit += ch.frame_bits() as usize;
            let (seq, mut slot) = (raw >> 2, raw & 3);
            if let Channel::CachedQuaternion { index } = &mut ch {
                *index = slot;
                slot = if kind == 7 { 3 } else { 4 };
            }
            channels.push((seq, slot, ch));
        }
        if n % 4 != 0 {
            item += (4 - n % 4) * 2;
        }
    }

    let mut values = vec![vec![[0.0f32, 0.0, 0.0, 1.0]; frames]; tracks];
    for (seq, slots) in group(channels).into_iter().enumerate() {
        let Some(out) = values.get_mut(seq) else { break };
        for (f, v) in out.iter_mut().enumerate() {
            let frame_start = (frame_offset + frame_len * f) * 8;
            *v = evaluate(&slots, data, f, frame_start);
        }
    }
    Ok(Block { frames, values })
}

/// Channels per anim sequence, by slot.
fn group(channels: Vec<(usize, usize, Channel)>) -> Vec<Vec<Option<Channel>>> {
    let count = channels.iter().map(|(s, ..)| s + 1).max().unwrap_or(0);
    let mut out: Vec<Vec<Option<Channel>>> = vec![Vec::new(); count];
    for (seq, slot, ch) in channels {
        let slots = &mut out[seq];
        if slots.len() <= slot {
            slots.resize(slot + 1, None);
        }
        slots[slot] = Some(ch);
    }
    out
}

/// Value of one anim sequence at a frame: a vector, or a quaternion when a
/// cached-quaternion channel derives the fourth component.
fn evaluate(slots: &[Option<Channel>], data: &[u8], frame: usize, frame_start: usize) -> [f32; 4] {
    let cached = slots.iter().flatten().find_map(|c| match c {
        Channel::CachedQuaternion { index } => Some(*index),
        _ => None,
    });
    if let Some(index) = cached {
        let xyz: Vec<f32> = (0..3)
            .map(|i| slots.get(i).and_then(|c| c.as_ref()).map_or(0.0, |c| c.float(data, frame, frame_start)))
            .collect();
        let w = (1.0 - (xyz[0] * xyz[0] + xyz[1] * xyz[1] + xyz[2] * xyz[2])).max(0.0).sqrt();
        let mut q = [0.0; 4];
        let mut rest = xyz.into_iter();
        for (i, c) in q.iter_mut().enumerate() {
            *c = if i == index.min(3) { w } else { rest.next().unwrap_or(0.0) };
        }
        return normalize(q);
    }
    let mut out = [0.0f32, 0.0, 0.0, 1.0];
    let mut n = 0;
    for ch in slots.iter().flatten() {
        match ch {
            Channel::StaticQuaternion(q) => {
                out = *q;
                n = 4;
            }
            Channel::StaticVector3(v) => {
                for (i, x) in v.iter().enumerate() {
                    if n + i < 4 {
                        out[n + i] = *x;
                    }
                }
                n += 3;
            }
            other => {
                if n < 4 {
                    out[n] = other.float(data, frame, frame_start);
                }
                n += 1;
            }
        }
        if n >= 4 {
            break;
        }
    }
    out
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l > 1e-6 { q.map(|c| c / l) } else { [0.0, 0.0, 0.0, 1.0] }
}

fn read_animation(res: &Resource, ptr: u64) -> Result<Animation, ResourceError> {
    let frames = res.u16(ptr + 0x14)? as usize;
    let frames_per_block = res.u16(ptr + 0x16)? as usize;
    let duration = f32::from_bits(res.u32(ptr + 0x18)?);
    let seq_ptr = res.u64(ptr + 0x40)?;
    let seq_count = res.u16(ptr + 0x48)? as u64;
    let track_ptr = res.u64(ptr + 0x50)?;
    let track_count = res.u16(ptr + 0x58)? as u64;
    if seq_count > 4096 || track_count > 4096 {
        return Err(bad("implausible animation"));
    }
    let tracks: Vec<BoneTrack> = (0..track_count)
        .map(|i| {
            let v = res.u32(track_ptr + i * 4)?;
            Ok(BoneTrack { bone: v as u16, format: (v >> 16) as u8, track: (v >> 24) as u8 })
        })
        .collect::<Result<_, ResourceError>>()?;
    let blocks =
        (0..seq_count).map(|i| read_block(res, res.u64(seq_ptr + i * 8)?, tracks.len())).collect::<Result<_, _>>()?;
    Ok(Animation { frames, frames_per_block, duration, tracks, blocks })
}

impl Animation {
    /// Value of track `t` at a (fractional) frame: linear interpolation of
    /// vectors, normalized lerp of quaternions.
    pub fn value(&self, t: usize, frame: f32) -> Option<[f32; 4]> {
        let f = frame.clamp(0.0, (self.frames.max(1) - 1) as f32);
        let (f0, frac) = (f.floor() as usize, f.fract());
        let a = self.frame_value(t, f0)?;
        let b = self.frame_value(t, (f0 + 1).min(self.frames.saturating_sub(1))).unwrap_or(a);
        let quat = self.tracks.get(t).is_some_and(|tr| tr.format == 1);
        Some(if quat {
            // Shortest path.
            let dot: f32 = (0..4).map(|i| a[i] * b[i]).sum();
            let b = if dot < 0.0 { b.map(|c| -c) } else { b };
            normalize(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * frac))
        } else {
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * frac)
        })
    }

    fn frame_value(&self, t: usize, frame: usize) -> Option<[f32; 4]> {
        // Blocks hold consecutive frames; a block's last frame repeats as
        // the next block's first when they overlap.
        let mut start = 0;
        for (i, b) in self.blocks.iter().enumerate() {
            let last = i + 1 == self.blocks.len();
            let span = if last || b.frames == 0 { b.frames } else { b.frames - 1 };
            if frame < start + span.max(1) || last {
                let local = (frame - start.min(frame)).min(b.frames.saturating_sub(1));
                return b.values.get(t)?.get(local).copied();
            }
            start += span;
        }
        None
    }

    /// The frame (fractional) shown at `time` seconds.
    pub fn frame_at(&self, time: f32) -> f32 {
        if self.duration <= 0.0 || self.frames < 2 {
            return 0.0;
        }
        (time / self.duration).clamp(0.0, 1.0) * (self.frames - 1) as f32
    }
}

/// Parses a `.ycd` file.
pub fn parse_ycd(res: &Resource) -> Result<ClipDictionary, ResourceError> {
    let root = Resource::ROOT;
    // Animations, by their address (clips refer to them by pointer).
    let mut animations: Vec<Animation> = Vec::new();
    let mut anim_ptrs: Vec<u64> = Vec::new();
    let map = res.u64(root + 0x18)?;
    if map != 0 {
        let buckets = res.u64(map + 0x18)?;
        let count = res.u16(map + 0x20)? as u64;
        for b in 0..count.min(65536) {
            let mut entry = res.u64(buckets + b * 8)?;
            let mut guard = 0;
            while entry != 0 && guard < 100_000 {
                let anim = res.u64(entry + 0x08)?;
                if anim != 0 && !anim_ptrs.contains(&anim) {
                    animations.push(read_animation(res, anim)?);
                    anim_ptrs.push(anim);
                }
                entry = res.u64(entry + 0x10)?;
                guard += 1;
            }
        }
    }
    let index_of = |ptr: u64| anim_ptrs.iter().position(|&p| p == ptr);

    let mut clips = Vec::new();
    let buckets = res.u64(root + 0x28)?;
    let count = res.u16(root + 0x30)? as u64;
    for b in 0..count.min(65536) {
        let mut entry = res.u64(buckets + b * 8)?;
        let mut guard = 0;
        while entry != 0 && guard < 100_000 {
            let clip = res.u64(entry + 0x08)?;
            if clip != 0 {
                let name_ptr = res.u64(clip + 0x18)?;
                let name = if name_ptr != 0 { res.string(name_ptr).unwrap_or_default() } else { String::new() };
                let name = name.strip_prefix("pack:/").unwrap_or(&name).trim_end_matches(".clip").to_owned();
                let mut parts = Vec::new();
                match res.u8(clip + 0x10)? {
                    1 => {
                        let anim = res.u64(clip + 0x50)?;
                        if let Some(animation) = index_of(anim) {
                            parts.push(ClipPart {
                                animation,
                                start: f32::from_bits(res.u32(clip + 0x58)?),
                                end: f32::from_bits(res.u32(clip + 0x5C)?),
                                rate: f32::from_bits(res.u32(clip + 0x60)?),
                            });
                        }
                    }
                    2 => {
                        let list = res.u64(clip + 0x50)?;
                        let n = res.u16(clip + 0x58)? as u64;
                        for i in 0..n.min(256) {
                            let e = list + i * 0x18;
                            if let Some(animation) = index_of(res.u64(e + 0x10)?) {
                                parts.push(ClipPart {
                                    animation,
                                    start: f32::from_bits(res.u32(e)?),
                                    end: f32::from_bits(res.u32(e + 4)?),
                                    rate: f32::from_bits(res.u32(e + 8)?),
                                });
                            }
                        }
                    }
                    _ => {}
                }
                clips.push(Clip { name, parts });
            }
            entry = res.u64(entry + 0x10)?;
            guard += 1;
        }
    }
    clips.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ClipDictionary { clips, animations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader() {
        let data = [0b1010_1100u8, 0b0000_0011];
        assert_eq!(bits(&data, 0, 4), 0b1100);
        assert_eq!(bits(&data, 2, 8), 0b1110_1011);
        assert_eq!(bits(&data, 20, 8), 0, "past the end reads zero");
    }

    #[test]
    fn cached_quaternion_inserts_component() {
        // x, y, z = 0, 0.6, 0; derived component goes to w (index 3).
        let slots = vec![
            Some(Channel::StaticFloat(0.0)),
            Some(Channel::StaticFloat(0.6)),
            Some(Channel::StaticFloat(0.0)),
            Some(Channel::CachedQuaternion { index: 3 }),
        ];
        let q = evaluate(&slots, &[], 0, 0);
        assert!((q[1] - 0.6).abs() < 1e-6 && (q[3] - 0.8).abs() < 1e-6, "{q:?}");
    }
}
