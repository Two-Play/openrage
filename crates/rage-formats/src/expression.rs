//! Expression dictionaries (`.yed`): small programs per ped that move the
//! face and helper bones from an animation's control values.
//!
//! Cutscene and dialogue clips do not move the jaw, lips or eyelids
//! directly. They animate facial control values (tracks of type 24 on
//! controller ids, not skeleton bones), and the ped's expressions turn those
//! into bone rotations and translations every frame. The same expressions
//! drive helper bones such as finger bulges and arm roll bones.
//!
//! Layout as documented by CodeWalker (MIT licence, see NOTICE.md); the
//! instruction meanings are this crate's reading of how the programs use them.

use rage_resource::{Resource, ResourceError};

/// One instruction of an expression stream (a stack machine of 4-float vectors).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    End,
    Pop,
    Dup,
    /// Pushes a constant.
    Push([f32; 4]),
    /// Reads or writes a track: `kind` is the opcode (0x06..0x0A get,
    /// 0x20 valid, 0x23, 0x26..0x2A set); `track` indexes [`Expression::tracks`].
    Track {
        kind: u8,
        track: u16,
        component: u8,
        defaults: bool,
    },
    GetVariable(u32),
    SetVariable(u32),
    /// Skips `target` instructions after the jump; `kind` 0x2B always, 0x2C
    /// if the top of the stack is false (zero), 0x2D if it is true. The
    /// condition stays on the stack.
    Jump {
        kind: u8,
        target: usize,
    },
    /// Weighted, piecewise-linear sum of track components (one per output
    /// component), pushed as a vector or as a rotation from Euler angles.
    Blend {
        quaternion: bool,
        sources: Vec<BlendSource>,
    },
    /// A spring that makes a bone lag behind (hair, flesh); not simulated.
    Spring,
    /// Aims a bone at a point: pops the target position, a rotation whose
    /// up axis to keep and the bone's own position; pushes the rotation.
    /// Axes: 0..2 positive x, y, z; 3..5 negative.
    LookAt {
        look_axis: u32,
        up_axis: u32,
    },
    /// Every other operation, with no operands (arithmetic, comparisons, …).
    Simple(u8),
    /// RDR2: pops a value into a numbered register (read by [`Op::LinearBlend`]).
    Store(u32),
    /// RDR2: a weighted sum of registers per output axis, plus a constant:
    /// `x = offset.x + Σ weights[g][0][lane] · register[registers[4g + lane]]`,
    /// likewise y and z (w is the offset's).
    LinearBlend {
        offset: [f32; 4],
        /// Per group of four lanes: the weights for x, y and z.
        groups: Vec<[[f32; 4]; 3]>,
        /// The register each lane reads (four per group).
        registers: Vec<u16>,
    },
    /// RDR2: an operation whose meaning is not known, only how many values
    /// it takes from the stack and gives back (those are unknown: NaN).
    Unknown {
        code: u8,
        pops: u8,
        pushes: u8,
    },
}

/// One input of a blend: the track component it reads and how it maps the
/// input to each of x, y and z.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendSource {
    pub track: u16,
    /// Component of the input value (0 = x … 3 = w).
    pub component: u8,
    /// Per output axis: weights and offsets per segment, and the input
    /// thresholds between segments (one fewer).
    pub axes: [BlendAxis; 3],
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlendAxis {
    pub weights: Vec<f32>,
    pub offsets: Vec<f32>,
    pub thresholds: Vec<f32>,
}

impl BlendAxis {
    /// The axis's contribution for an input value: the segment's line.
    pub fn eval(&self, x: f32) -> f32 {
        let segment = self.thresholds.iter().take_while(|t| x >= **t).count();
        let w = self.weights.get(segment).copied().unwrap_or(0.0);
        let o = self.offsets.get(segment).copied().unwrap_or(0.0);
        w * x + o
    }
}

/// A track an expression reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Track {
    /// Bone tag, or facial controller id.
    pub bone: u16,
    /// Track type (0 translation, 1 rotation, 2 scale, 24 facial control, …).
    pub track: u8,
    /// 0 vector, 1 quaternion, 2 float.
    pub format: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub name: u32,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub name: String,
    pub tracks: Vec<Track>,
    pub streams: Vec<Stream>,
    pub variables: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ExpressionDictionary {
    pub expressions: Vec<Expression>,
}

impl ExpressionDictionary {
    /// The expression of a ped model, by name (`player_zero`, …).
    pub fn find(&self, name: &str) -> Option<&Expression> {
        self.expressions.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }
}

/// A byte reader over a stream's operand data.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ResourceError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or_else(|| short("operand data"))?;
        let s = &self.data[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, ResourceError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ResourceError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }
    fn u32(&mut self) -> Result<u32, ResourceError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn f32(&mut self) -> Result<f32, ResourceError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn vec4(&mut self) -> Result<[f32; 4], ResourceError> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
}

fn short(what: &str) -> ResourceError {
    ResourceError::Format(format!("expression {what} too short"))
}

fn read_blend(r1: &mut Reader, quaternion: bool) -> Result<Op, ResourceError> {
    let _byte_length = r1.u32()?;
    let count = r1.u32()? as usize;
    let weights = r1.u32()?.max(1) as usize;
    let _ = r1.u32()?;
    if count > 4096 || weights > 16 {
        return Err(ResourceError::Format("expression blend too large".into()));
    }
    let infos: Vec<(u16, u16)> =
        (0..count).map(|_| Ok((r1.u16()?, r1.u16()?))).collect::<Result<_, ResourceError>>()?;
    // Values are stored four sources at a time (one per vector lane).
    let per_group = 6 + 9 * (weights - 1);
    let values: Vec<[f32; 4]> = (0..count.div_ceil(4) * per_group).map(|_| r1.vec4()).collect::<Result<_, _>>()?;
    let sources = infos
        .iter()
        .enumerate()
        .map(|(i, &(track, offset))| {
            let (base, lane) = ((i / 4) * per_group, i % 4);
            let v = |k: usize| values.get(base + k).map_or(0.0, |v| v[lane]);
            let mut axes: [BlendAxis; 3] = Default::default();
            for (a, axis) in axes.iter_mut().enumerate() {
                axis.weights.push(v(a));
                axis.offsets.push(v(3 + a));
                for n in 1..weights {
                    let b = 6 + 9 * (n - 1);
                    axis.thresholds.push(v(b + a));
                    axis.weights.push(v(b + 3 + a));
                    axis.offsets.push(v(b + 6 + a));
                }
            }
            BlendSource { track, component: (offset / 4) as u8, axes }
        })
        .collect();
    Ok(Op::Blend { quaternion, sources })
}

fn read_ops(d1: &[u8], d2: &[u8], d3: &[u8]) -> Result<Vec<Op>, ResourceError> {
    let (mut r1, mut r2) = (Reader { data: d1, at: 0 }, Reader { data: d2, at: 0 });
    let mut ops = Vec::with_capacity(d3.len());
    for &code in d3 {
        let op = match code {
            0x00 => Op::End,
            0x01 => Op::Pop,
            0x02 => Op::Dup,
            0x03 => Op::Push([0.0; 4]),
            0x04 => Op::Push([1.0; 4]),
            0x05 => Op::Push([r2.f32()?; 4]),
            0x0B => Op::Push(r1.vec4()?),
            0x06..=0x0A | 0x20 | 0x23 | 0x26..=0x2A => {
                let track = r2.u16()?;
                let _bone = r2.u16()?;
                let _track_type = r2.u8()?;
                let _format = r2.u8()?;
                let component = r2.u8()?;
                let defaults = r2.u8()? != 0;
                Op::Track { kind: code, track, component, defaults }
            }
            0x42 | 0x43 => {
                let _hash = r2.u32()?;
                let index = r2.u32()?;
                if code == 0x42 { Op::GetVariable(index) } else { Op::SetVariable(index) }
            }
            0x2B..=0x2D => {
                let _ = (r2.u32()?, r2.u32()?);
                Op::Jump { kind: code, target: r2.u32()? as usize }
            }
            0x0E => {
                r1.take(160 + 16)?;
                Op::Spring
            }
            0x3E => {
                let _offset = r1.vec4()?;
                let look_axis = r1.u32()?;
                let up_axis = r1.u32()?;
                let _ = (r1.u32()?, r1.u32()?);
                Op::LookAt { look_axis, up_axis }
            }
            0x44 | 0x45 => read_blend(&mut r1, code == 0x45)?,
            other => Op::Simple(other),
        };
        ops.push(op);
    }
    Ok(ops)
}

/// RDR2 operations: (operand bytes in the vector data, in the other data,
/// values popped, values pushed) for those not decoded on their own. Found by
/// how every one of the game's 1,603 expression streams uses its data.
fn rdr2_shape(code: u8) -> Option<(usize, usize, u8, u8)> {
    Some(match code {
        // Operations of one value whose meaning is not known.
        0x0C | 0x0D | 0x0E | 0x14 => (0, 0, 1, 1),
        0x24 => (0, 8, 1, 1),
        0x3E => (0, 0, 3, 1),
        0x4A | 0x4B | 0x4E | 0x54 | 0x6E => (0, 0, 0, 0),
        0x4F => (0, 0, 2, 1),
        // Values from elsewhere (ped state, other bones…).
        0x45 | 0x57 | 0x67 | 0x6D | 0x71 | 0x72 => (0, 8, 0, 1),
        0x46 | 0x56 => (0, 0, 0, 1),
        0x60 | 0x61 => (0, 12, 0, 1),
        0x62 => (80, 0, 0, 1),
        0x65 => (0, 16, 0, 1),
        0x59 | 0x5A => (0, 4, 1, 0),
        0x5F => (0, 0, 1, 0),
        0x5E => (0, 12, 0, 0),
        0x5C | 0x5D => (32, 0, 4, 1),
        _ => return None,
    })
}

/// Reads the operations of a Red Dead Redemption 2 stream. Most keep their
/// GTA V meaning; jumps differ (0x2B always, 0x70 when the popped condition
/// is false), 0x67 also reads a variable, 0x52 stores into a register and
/// 0x50 sums registers (see [`Op::LinearBlend`]).
fn read_ops_rdr2(d1: &[u8], d2: &[u8], d3: &[u8]) -> Result<Vec<Op>, ResourceError> {
    let (mut r1, mut r2) = (Reader { data: d1, at: 0 }, Reader { data: d2, at: 0 });
    let mut ops = Vec::with_capacity(d3.len());
    for &code in d3 {
        let op = match code {
            0x00 => Op::End,
            0x01 => Op::Pop,
            0x02 => Op::Dup,
            0x03 => Op::Push([0.0; 4]),
            0x04 => Op::Push([1.0; 4]),
            0x05 => Op::Push([r2.f32()?; 4]),
            0x0B => Op::Push(r1.vec4()?),
            0x06..=0x0A | 0x20 | 0x23 | 0x26..=0x2A => {
                let track = r2.u16()?;
                let _bone = r2.u16()?;
                let _track_type = r2.u8()?;
                let _format = r2.u8()?;
                let component = r2.u8()?;
                let defaults = r2.u8()? != 0;
                Op::Track { kind: code, track, component, defaults }
            }
            0x42 | 0x43 | 0x67 => {
                let _hash = r2.u32()?;
                let index = r2.u32()?;
                if code == 0x43 { Op::SetVariable(index) } else { Op::GetVariable(index) }
            }
            0x2B | 0x2C | 0x2D | 0x70 => {
                let _ = (r2.u32()?, r2.u32()?);
                Op::Jump { kind: code, target: r2.u32()? as usize }
            }
            0x52 => Op::Store(r2.u32()?),
            0x50 => {
                let start = r1.at;
                let length = r1.u32()? as usize;
                let groups = r1.u32()? as usize;
                let _ = (r1.u32()?, r1.u32()?);
                if groups > 1024 || length != 32 + 48 * groups + 16 * groups.div_ceil(2) {
                    return Err(ResourceError::Format("expression register blend of unexpected size".into()));
                }
                let offset = r1.vec4()?;
                let groups_data = (0..groups)
                    .map(|_| Ok([r1.vec4()?, r1.vec4()?, r1.vec4()?]))
                    .collect::<Result<Vec<_>, ResourceError>>()?;
                let registers =
                    (0..groups.div_ceil(2) * 8).map(|_| r1.u16()).collect::<Result<Vec<_>, ResourceError>>()?;
                debug_assert_eq!(r1.at - start, length);
                Op::LinearBlend { offset, groups: groups_data, registers }
            }
            0x0F..=0x12 | 0x1B..=0x1F | 0x21 | 0x22 | 0x2E..=0x33 | 0x35..=0x3D | 0x40 | 0x48 | 0x49 => {
                Op::Simple(code)
            }
            other => {
                let Some((n1, n2, pops, pushes)) = rdr2_shape(other) else {
                    return Err(ResourceError::Format(format!("unknown RDR2 expression operation {other:#04x}")));
                };
                r1.take(n1)?;
                r2.take(n2)?;
                Op::Unknown { code: other, pops, pushes }
            }
        };
        ops.push(op);
    }
    Ok(ops)
}

/// (pointer, count) of a 64-bit resource list.
fn list(res: &Resource, at: u64) -> Result<(u64, u64), ResourceError> {
    Ok((res.u64(at)?, u64::from(res.u16(at + 8)?)))
}

fn read_expression(res: &Resource, e: u64) -> Result<Expression, ResourceError> {
    read_expression_at(res, e, 0)
}

/// An expression; RDR2's fields sit `shift` (0x10) bytes further on.
fn read_expression_at(res: &Resource, e: u64, shift: u64) -> Result<Expression, ResourceError> {
    let (streams_ptr, stream_count) = list(res, e + 0x20 + shift)?;
    let (tracks_ptr, track_count) = list(res, e + 0x30 + shift)?;
    let (_, variables) = list(res, e + 0x50 + shift)?;
    let name = res.string(res.u64(e + 0x60 + shift)?).unwrap_or_default();
    let tracks = (0..track_count)
        .map(|i| {
            let t = tracks_ptr + i * 4;
            Ok(Track { bone: res.u16(t)?, track: res.u8(t + 2)?, format: res.u8(t + 3)? & 0x7F })
        })
        .collect::<Result<_, ResourceError>>()?;
    let mut streams = Vec::with_capacity(stream_count as usize);
    for i in 0..stream_count {
        let s = res.u64(streams_ptr + i * 8)?;
        let name = res.u32(s)?;
        let (l1, l2, l3) = (res.u32(s + 4)? as usize, res.u32(s + 8)? as usize, res.u16(s + 12)? as usize);
        let d1 = res.bytes(s + 16, l1)?;
        let d2 = res.bytes(s + 16 + l1 as u64, l2)?;
        let d3 = res.bytes(s + 16 + (l1 + l2) as u64, l3)?;
        let ops = if shift == 0 { read_ops(d1, d2, d3)? } else { read_ops_rdr2(d1, d2, d3)? };
        streams.push(Stream { name, ops });
    }
    Ok(Expression { name, tracks, streams, variables: variables as usize })
}

/// Reads an expression dictionary (`.yed`).
pub fn parse(res: &Resource) -> Result<ExpressionDictionary, ResourceError> {
    let (ptr, count) = list(res, Resource::ROOT + 0x30)?;
    let expressions =
        (0..count.min(4096)).map(|i| read_expression(res, res.u64(ptr + i * 8)?)).collect::<Result<_, _>>()?;
    Ok(ExpressionDictionary { expressions })
}

/// Reads a Red Dead Redemption 2 expression dictionary (resource version
/// 30): the expressions are in a hash map (buckets at 0x48, their count at
/// 0x50; entries hold the name hash, the expression and the next entry), and
/// each expression's fields sit 0x10 further on than in GTA V. Expressions
/// with operations not understood yet (a few with variable-size operands)
/// are left out.
pub fn parse_rdr2(res: &Resource) -> Result<ExpressionDictionary, ResourceError> {
    let buckets = res.u64(Resource::ROOT + 0x48)?;
    let count = u64::from(res.u16(Resource::ROOT + 0x50)?);
    let mut expressions = Vec::new();
    for b in 0..count.min(4096) {
        let mut entry = res.u64(buckets + b * 8)?;
        let mut guard = 0;
        while entry != 0 && guard < 4096 {
            // Expressions with operations not understood yet are left out.
            if let Ok(e) = read_expression_at(res, res.u64(entry + 8)?, 0x10) {
                expressions.push(e);
            }
            entry = res.u64(entry + 0x10)?;
            guard += 1;
        }
    }
    Ok(ExpressionDictionary { expressions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_operands_from_both_data_streams() {
        let mut d1 = Vec::new();
        for v in [1.0f32, 2.0, 3.0, 4.0] {
            d1.extend(v.to_le_bytes());
        }
        let mut d2 = Vec::new();
        d2.extend(0.5f32.to_le_bytes());
        d2.extend([3, 0, 0x34, 0x12, 24, 2, 0, 0]); // track 3, bone 0x1234, facial float
        let ops = read_ops(&d1, &d2, &[0x0B, 0x05, 0x06, 0x30, 0x00]).unwrap();
        assert_eq!(ops[0], Op::Push([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(ops[1], Op::Push([0.5; 4]));
        assert_eq!(ops[2], Op::Track { kind: 0x06, track: 3, component: 0, defaults: false });
        assert_eq!(ops[3], Op::Simple(0x30));
        assert_eq!(ops[4], Op::End);
    }

    #[test]
    fn decodes_rdr2_operations() {
        // A register blend with one group: header, offset, x/y/z weights, lanes.
        let mut d1 = Vec::new();
        for w in [96u32, 1, 0, 0] {
            d1.extend(w.to_le_bytes());
        }
        for v in [0.0f32, 0.0, 0.0, 0.0, -0.5, -0.5, 0.0, 0.0, 0.0, 0.0, 0.7, 0.7, 0.0, 0.0, 0.0, 0.0] {
            d1.extend(v.to_le_bytes());
        }
        for lane in [1u16, 2, 3, 4, 0, 0, 0, 0] {
            d1.extend(lane.to_le_bytes());
        }
        let mut d2 = Vec::new();
        d2.extend(5u32.to_le_bytes()); // 0x52: register 5
        d2.extend([0, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0]); // 0x70: skip two operations
        d2.extend([0xBA, 0x16, 0x47, 0x4D, 0x10, 0, 0, 0]); // 0x67: variable 16
        let ops = read_ops_rdr2(&d1, &d2, &[0x52, 0x50, 0x70, 0x67, 0x0C, 0x00]).unwrap();
        assert_eq!(ops[0], Op::Store(5));
        let Op::LinearBlend { offset, groups, registers } = &ops[1] else { panic!("{:?}", ops[1]) };
        assert_eq!(*offset, [0.0; 4]);
        assert_eq!(groups[0][0], [-0.5, -0.5, 0.0, 0.0]);
        assert_eq!(groups[0][1], [0.0, 0.0, 0.7, 0.7]);
        assert_eq!(&registers[..4], &[1, 2, 3, 4]);
        assert_eq!(ops[2], Op::Jump { kind: 0x70, target: 2 });
        assert_eq!(ops[3], Op::GetVariable(16));
        assert_eq!(ops[4], Op::Unknown { code: 0x0C, pops: 1, pushes: 1 });
        // An operation whose operands are not understood stops the stream.
        assert!(read_ops_rdr2(&[], &[], &[0x74]).is_err());
    }

    #[test]
    fn blend_segments_are_piecewise_linear() {
        let axis = BlendAxis { weights: vec![1.0, 2.0], offsets: vec![0.0, -0.5], thresholds: vec![0.5] };
        assert_eq!(axis.eval(0.25), 0.25);
        assert_eq!(axis.eval(1.0), 1.5);
    }
}
