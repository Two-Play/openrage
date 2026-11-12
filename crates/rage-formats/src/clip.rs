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
    /// RDR2: the animation's flags (0 for GTA V).
    pub flags: u16,
    pub frames: usize,
    pub frames_per_block: usize,
    pub duration: f32,
    pub tracks: Vec<BoneTrack>,
    blocks: Vec<Block>,
}

impl Animation {
    /// An animation without tracks: every bone keeps its rest pose.
    pub fn empty() -> Animation {
        Animation { flags: 0, frames: 1, frames_per_block: 1, duration: 0.0, tracks: Vec::new(), blocks: Vec::new() }
    }
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
    /// The parts play one after another (a cutscene's sections joined by
    /// [`join_sections`]); otherwise the first part is the clip.
    pub sequential: bool,
}

impl ClipPart {
    fn duration(&self) -> f32 {
        (self.end - self.start).max(0.0) / self.rate.max(0.001)
    }
}

impl Clip {
    /// Length in seconds (the longest part, or all parts in a row).
    pub fn duration(&self) -> f32 {
        let parts = self.parts.iter().map(ClipPart::duration);
        if self.sequential { parts.sum() } else { parts.fold(0.0, f32::max) }
    }

    /// The part playing at `time` seconds into the clip, its index and the
    /// time in the part's animation.
    pub fn part_at(&self, time: f32) -> Option<(usize, &ClipPart, f32)> {
        if !self.sequential {
            let p = self.parts.first()?;
            return Some((0, p, p.start + time * p.rate));
        }
        let mut t = time.max(0.0);
        for (i, p) in self.parts.iter().enumerate() {
            let d = p.duration();
            if t < d || i + 1 == self.parts.len() {
                return Some((i, p, p.start + t.min(d) * p.rate));
            }
            t -= d;
        }
        None
    }
}

/// Joins a cutscene's sections (`name-0.ycd`, `name-1.ycd`, …, in order) into
/// one dictionary: one clip per actor (`csb_bogdan_dual-3` → `csb_bogdan_dual`)
/// whose parts are its clips of every section, played in a row. Sections an
/// actor is missing from are left out.
pub fn join_sections(sections: Vec<ClipDictionary>) -> ClipDictionary {
    let mut out = ClipDictionary::default();
    let mut names: Vec<String> = Vec::new();
    for section in sections {
        let base = out.animations.len();
        out.animations.extend(section.animations);
        for clip in section.clips {
            let name = match clip.name.rsplit_once('-') {
                Some((stem, n)) if n.chars().all(|c| c.is_ascii_digit()) => stem.to_owned(),
                _ => clip.name.clone(),
            };
            let Some(first) = clip.parts.into_iter().next() else { continue };
            let part = ClipPart { animation: first.animation + base, ..first };
            match names.iter().position(|n| *n == name) {
                Some(i) => out.clips[i].parts.push(part),
                None => {
                    names.push(name.clone());
                    out.clips.push(Clip { name, parts: vec![part], sequential: true });
                }
            }
        }
    }
    out
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
    /// Whether the channel gives one float component.
    fn is_float(&self) -> bool {
        matches!(
            self,
            Channel::StaticFloat(_)
                | Channel::RawFloat { .. }
                | Channel::QuantizeFloat { .. }
                | Channel::IndirectQuantizeFloat { .. }
                | Channel::LinearFloat(_)
        )
    }

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
        6 | 9 => {
            // `num_ints` covers the whole channel, this 16-byte header included.
            let start = c.pos;
            let num_ints = c.u32()? as usize;
            let counts = c.u32()?;
            let quantum = c.f32()?;
            let offset = c.f32()?;
            let values = if kind == 9 {
                chunk_starts(c.data, c.pos * 8, frames, counts, quantum, offset)
            } else {
                linear_float(c.data, c.pos * 8, frames, counts, quantum, offset)
            };
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
/// RDR2's kind-9 channels: the coding between chunk starts is not known
/// yet, so the values are interpolated linearly between the exact values
/// stored at the start of every 64-frame chunk (the last one is held).
fn chunk_starts(data: &[u8], base: usize, frames: usize, counts: u32, quantum: f32, offset: f32) -> Vec<f32> {
    const CHUNK: usize = 64;
    let (c1, c2) = (counts & 0xFF, (counts >> 8) & 0xFF);
    let chunks = frames.div_ceil(CHUNK).max(1);
    let starts: Vec<f32> = (0..chunks)
        .map(|i| bits(data, base + chunks * c1 as usize + i * c2 as usize, c2.min(32)) as f32 * quantum + offset)
        .collect();
    (0..frames)
        .map(|f| {
            let (i, t) = (f / CHUNK, (f % CHUNK) as f32 / CHUNK as f32);
            match (starts.get(i), starts.get(i + 1)) {
                (Some(a), Some(b)) => a + (b - a) * t,
                (Some(a), None) => *a,
                _ => 0.0,
            }
        })
        .collect()
}

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
/// RDR2 has a tenth channel kind after GTA V's nine (kind 9 here): floats
/// with the parameters of GTA V's linear floats but another coding.
const RDR2_KINDS: [u16; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

fn read_block(res: &Resource, ptr: u64, tracks: usize, rdr2: bool) -> Result<Block, ResourceError> {
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
    let kinds = if rdr2 { 10 } else { 9 };
    let mut item = list + kinds * 2;
    let mut frame_bit = 0usize;
    for stored in 0..kinds as u16 {
        let n = count(stored as usize)?;
        let kind = if rdr2 { RDR2_KINDS[stored as usize] } else { stored };

        for _ in 0..n {
            let raw = data.get(item..item + 2).ok_or_else(|| bad("channel item outside the block"))?;
            let raw = u16::from_le_bytes([raw[0], raw[1]]) as usize;
            item += 2;
            let mut ch = read_channel(kind, &mut params, frames)?;
            ch.set_bit(frame_bit);
            frame_bit += ch.frame_bits() as usize;
            let (seq, mut slot) = (raw >> 2, raw & 3);
            if let Channel::CachedQuaternion { index } = &mut ch {
                // RDR2 always derives w; its low bits mean something else there.
                *index = if rdr2 { 3 } else { slot };
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
    // All four components stored: nothing to derive (RDR2 marks such tracks
    // with a cached-quaternion channel too).
    let explicit = (0..4).all(|i| slots.get(i).and_then(|c| c.as_ref()).is_some_and(|c| c.is_float()));
    let cached = slots.iter().flatten().find_map(|c| match c {
        Channel::CachedQuaternion { index } if !explicit => Some(*index),
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
    let blocks = (0..seq_count)
        .map(|i| read_block(res, res.u64(seq_ptr + i * 8)?, tracks.len(), false))
        .collect::<Result<_, _>>()?;
    Ok(Animation { flags: 0, frames, frames_per_block, duration, tracks, blocks })
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

    /// Frames per sequence block (for checks).
    pub fn block_frames(&self) -> Vec<usize> {
        self.blocks.iter().map(|b| b.frames).collect()
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
    if res.version == RDR2_VERSION {
        return parse_ycd_rdr2(res);
    }
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
                clips.push(Clip { name, parts, sequential: false });
            }
            entry = res.u64(entry + 0x10)?;
            guard += 1;
        }
    }
    clips.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ClipDictionary { clips, animations })
}

/// A Red Dead Redemption 2 animation (`crAnimation` in clip dictionaries
/// of version 58): the same sequence blocks as GTA V's, in a larger header.
fn read_animation_rdr2(res: &Resource, ptr: u64) -> Result<Animation, ResourceError> {
    let frames = res.u16(ptr + 0x24)? as usize;
    let frames_per_block = res.u16(ptr + 0x26)? as usize;
    let duration = f32::from_bits(res.u32(ptr + 0x28)?);
    let seq_ptr = res.u64(ptr + 0x50)?;
    let seq_count = res.u16(ptr + 0x58)? as u64;
    let track_ptr = res.u64(ptr + 0x60)?;
    let track_count = res.u16(ptr + 0x68)? as u64;
    if seq_count > 4096 || track_count > 4096 {
        return Err(bad("implausible animation"));
    }
    let tracks: Vec<BoneTrack> = (0..track_count)
        .map(|i| {
            let v = res.u32(track_ptr + i * 4)?;
            Ok(BoneTrack { bone: v as u16, format: (v >> 16) as u8, track: (v >> 24) as u8 })
        })
        .collect::<Result<_, ResourceError>>()?;
    let blocks = (0..seq_count)
        .map(|i| read_block(res, res.u64(seq_ptr + i * 8)?, tracks.len(), true))
        .collect::<Result<_, _>>()?;
    Ok(Animation { flags: res.u16(ptr + 0x20)?, frames, frames_per_block, duration, tracks, blocks })
}

/// Walks a hash map (bucket pointers, entries chained at +0x10) and returns
/// the object pointer (+0x08) of every entry.
fn map_objects(res: &Resource, buckets: u64, count: u64) -> Result<Vec<u64>, ResourceError> {
    let mut out = Vec::new();
    for b in 0..count.min(65536) {
        let mut entry = res.u64(buckets + b * 8)?;
        let mut guard = 0;
        while entry != 0 && guard < 100_000 {
            let object = res.u64(entry + 0x08)?;
            if object != 0 {
                out.push(object);
            }
            entry = res.u64(entry + 0x10)?;
            guard += 1;
        }
    }
    Ok(out)
}

/// Parses a Red Dead Redemption 2 `.ycd` (version 58): a clip dictionary
/// with its animation dictionaries embedded. Clips refer to animations by
/// pointer; each clip has a list of animation parts (pointer, start, end, rate).
pub fn parse_ycd_rdr2(res: &Resource) -> Result<ClipDictionary, ResourceError> {
    let root = Resource::ROOT;
    let mut animations = Vec::new();
    let mut anim_ptrs = Vec::new();
    let dicts = res.u64(root + 0x70)?;
    let dict_count = res.u16(root + 0x78)? as u64;
    for d in 0..dict_count.min(256) {
        let dict = res.u64(dicts + d * 16)?;
        if dict == 0 {
            continue;
        }
        let (buckets, count) = (res.u64(dict + 0x48)?, res.u16(dict + 0x50)? as u64);
        for anim in map_objects(res, buckets, count)? {
            if !anim_ptrs.contains(&anim) {
                animations.push(read_animation_rdr2(res, anim)?);
                anim_ptrs.push(anim);
            }
        }
    }
    let index_of = |ptr: u64| anim_ptrs.iter().position(|&p| p == ptr);
    let mut clips = Vec::new();
    for clip in map_objects(res, res.u64(root + 0x48)?, res.u16(root + 0x50)? as u64)? {
        let name_ptr = res.u64(clip + 0x28)?;
        let name = if name_ptr != 0 { res.string(name_ptr).unwrap_or_default() } else { String::new() };
        let name = name.strip_prefix("pack:/").unwrap_or(&name).trim_end_matches(".clip").to_owned();
        // Parts (played one after another): each a list of animation
        // layers and a duration. The first layer is the part's animation:
        // pointer, then start, end and rate at 0x28.
        let list = res.u64(clip + 0x68)?;
        let n = res.u16(clip + 0x70)? as u64;
        let mut parts = Vec::new();
        for i in 0..n.min(256) {
            let part = list + i * RDR2_CLIP_PART;
            let (layers, layer_count) = (res.u64(part)?, res.u16(part + 8)?);
            if layers == 0 || layer_count == 0 {
                continue;
            }
            if let Some(animation) = index_of(res.u64(layers)?) {
                parts.push(ClipPart {
                    animation,
                    start: f32::from_bits(res.u32(layers + 0x28)?),
                    end: f32::from_bits(res.u32(layers + 0x2C)?),
                    rate: f32::from_bits(res.u32(layers + 0x30)?),
                });
            }
        }
        let sequential = parts.len() > 1;
        clips.push(Clip { name, parts, sequential });
    }
    clips.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ClipDictionary { clips, animations })
}

/// Resource version of Red Dead Redemption 2's clip dictionaries.
pub const RDR2_VERSION: u32 = 58;

/// Size of a part of an RDR2 clip (layer list, count, capacity, duration).
const RDR2_CLIP_PART: u64 = 0x18;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdr2_kind9_interpolates_chunk_starts() {
        // Two chunks (100 frames): 4-bit offsets, then 8-bit starts 10 and 20.
        let (c1, c2) = (4u32, 8u32);
        let mut v: u64 = 0;
        v |= 10 << (2 * c1);
        v |= 20 << (2 * c1 + c2);
        let data = v.to_le_bytes();
        let values = chunk_starts(&data, 0, 100, c1 | c2 << 8, 1.0, 0.0);
        assert_eq!(values.len(), 100);
        assert_eq!(values[0], 10.0);
        assert_eq!(values[32], 15.0);
        assert_eq!(values[64], 20.0);
        assert_eq!(values[99], 20.0);
    }

    #[test]
    fn joins_cutscene_sections() {
        let section = |n: usize| ClipDictionary {
            clips: vec![
                Clip {
                    name: format!("actor_dual-{n}"),
                    parts: vec![ClipPart { animation: 0, start: 0.0, end: 4.0, rate: 1.0 }],
                    sequential: false,
                },
                Clip {
                    name: format!("prop-{n}"),
                    parts: vec![ClipPart { animation: 1, start: 0.0, end: 4.0, rate: 1.0 }],
                    sequential: false,
                },
            ],
            animations: Vec::new(),
        };
        let joined = join_sections(vec![section(0), section(1), section(2)]);
        assert_eq!(joined.clips.len(), 2);
        let actor = &joined.clips[0];
        assert_eq!(actor.name, "actor_dual");
        assert_eq!(actor.duration(), 12.0);
        let (i, part, at) = actor.part_at(9.0).unwrap();
        assert_eq!((i, part.animation, at), (2, 0, 1.0));
        assert_eq!(actor.part_at(20.0).unwrap().0, 2);
    }

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
