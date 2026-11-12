//! Evaluates a ped's expressions (`.yed`): every frame they turn facial
//! control values from the animation into face bone movements and set
//! helper bones (finger bulges, roll bones) from the main bones.
//!
//! The programs run on a stack of 4-float vectors. Rotations are
//! quaternions `x, y, z, w`; components of a rotation are read and written
//! as Euler angles in radians.

use std::collections::HashMap;

use rage_formats::clip::Animation;
use rage_formats::drawable::Skeleton;
use rage_formats::expression::{Expression, Op, Track};

/// A bone's local pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Local {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
enum Slot {
    /// A skeleton bone's translation (0), rotation (1) or scale (2).
    Bone(usize, u8),
    /// A value shared between the expressions of a frame (facial controls
    /// and the values computed from them): its key (bone id, track type),
    /// the animation track that provides it, and its format.
    Control((u16, u8), Option<usize>, u8),
}

/// Values the expressions of one frame share.
pub type Controls = HashMap<(u16, u8), [f32; 4]>;

/// One expression bound to a skeleton and an animation.
pub struct Program {
    ops: Vec<Op>,
    slots: Vec<Slot>,
    variables: usize,
}

const STACK_LIMIT: usize = 4096;
const STEP_LIMIT: usize = 100_000;

impl Program {
    /// Binds every stream of `expression` (in order) to `skeleton` and `animation`.
    pub fn new(expression: &Expression, skeleton: &Skeleton, animation: &Animation) -> Program {
        let slots = expression
            .tracks
            .iter()
            .map(|t: &Track| match (t.track, skeleton.find_tag(t.bone)) {
                (0..=2, Some(b)) => Slot::Bone(b, t.track),
                _ => Slot::Control(
                    (t.bone, t.track),
                    animation.tracks.iter().position(|a| a.bone == t.bone && a.track == t.track),
                    t.format,
                ),
            })
            .collect();
        let mut ops = Vec::new();
        for s in &expression.streams {
            // Streams run one after the other. A jump skips `target`
            // instructions after itself; conditional jumps leave the
            // condition on the stack (both paths pop it).
            let base = ops.len();
            ops.extend(s.ops.iter().enumerate().filter(|(_, o)| !matches!(o, Op::End)).map(|(i, o)| match o {
                Op::Jump { kind, target } => Op::Jump { kind: *kind, target: base + i + 1 + target },
                o => o.clone(),
            }));
        }
        Program { ops, slots, variables: expression.variables }
    }

    /// Runs the program on the pose (local bone transforms; `rest` is the
    /// skeleton's own pose) at an animation frame.
    /// Returns how many values are left on the stack (0 for a well-formed program).
    pub fn run(
        &self,
        pose: &mut [Local],
        rest: &[Local],
        parents: &[Option<usize>],
        animation: &Animation,
        frame: f32,
        controls: &mut Controls,
    ) -> usize {
        let mut stack: Vec<[f32; 4]> = Vec::with_capacity(32);
        let mut variables = vec![[0.0f32; 4]; self.variables];
        let mut pc = 0;
        let mut steps = 0;
        let pop = |stack: &mut Vec<[f32; 4]>| stack.pop().unwrap_or([0.0; 4]);
        while pc < self.ops.len() && steps < STEP_LIMIT && stack.len() < STACK_LIMIT {
            steps += 1;
            let op = &self.ops[pc];
            pc += 1;
            match op {
                Op::End | Op::Spring => {}
                Op::LookAt { look_axis, up_axis } => {
                    // Own position, the frame whose up axis to keep, the target.
                    let own = pop(&mut stack);
                    let frame_rot = pop(&mut stack);
                    let target = pop(&mut stack);
                    stack.push(look_at(target, frame_rot, own, *look_axis, *up_axis));
                }
                Op::Pop => {
                    stack.pop();
                }
                Op::Dup => stack.push(stack.last().copied().unwrap_or([0.0; 4])),
                Op::Push(v) => stack.push(*v),
                Op::GetVariable(i) => stack.push(variables.get(*i as usize).copied().unwrap_or([0.0; 4])),
                Op::SetVariable(i) => {
                    let v = pop(&mut stack);
                    if let Some(slot) = variables.get_mut(*i as usize) {
                        *slot = v;
                    }
                }
                Op::Jump { kind, target } => {
                    let top = stack.last().map_or(0.0, |v| v[0]);
                    let jump = match kind {
                        0x2C => top == 0.0,
                        0x2D => top != 0.0,
                        _ => true,
                    };
                    if jump {
                        pc = *target;
                    }
                }
                Op::Track { kind, track, component, .. } => {
                    let slot = self.slots.get(*track as usize).copied();
                    let c = *component as usize & 3;
                    match kind {
                        0x06..=0x09 => {
                            let offset = matches!(kind, 0x08 | 0x09);
                            let v = slot.map_or([0.0; 4], |s| get(s, offset, pose, rest, animation, frame, controls));
                            stack.push(if matches!(kind, 0x07 | 0x09) { component_of(slot, v, c) } else { v });
                        }
                        0x20 => {
                            let valid = match slot {
                                Some(Slot::Bone(..)) => true,
                                Some(Slot::Control(key, track, _)) => track.is_some() || controls.contains_key(&key),
                                None => false,
                            };
                            stack.push([if valid { 1.0 } else { 0.0 }; 4]);
                        }
                        // The bone's transform in the model: position or rotation.
                        0x0A => stack.push(match slot {
                            Some(Slot::Bone(b, kind)) => {
                                let (p, q) = object_transform(pose, parents, b);
                                if kind == 1 { q } else { v3(p) }
                            }
                            _ => [0.0, 0.0, 0.0, 1.0],
                        }),
                        // A rotation in the model's space into the bone's parent space.
                        0x23 => {
                            let q = pop(&mut stack);
                            let parent = match slot {
                                Some(Slot::Bone(b, _)) => parents.get(b).copied().flatten(),
                                _ => None,
                            };
                            let pq = parent.map_or([0.0, 0.0, 0.0, 1.0], |p| object_transform(pose, parents, p).1);
                            stack.push(normalize(quat_mul(conjugate(pq), q)));
                        }
                        0x26..=0x29 => {
                            let v = pop(&mut stack);
                            if let Some(s) = slot {
                                let offset = matches!(kind, 0x28 | 0x29);
                                let value = if matches!(kind, 0x27 | 0x29) {
                                    let current = get(s, offset, pose, rest, animation, frame, controls);
                                    with_component(Some(s), current, c, v[0])
                                } else {
                                    v
                                };
                                set(s, offset, value, pose, rest, controls);
                            }
                        }
                        0x2A => {
                            stack.pop();
                        }
                        _ => {}
                    }
                }
                Op::Blend { quaternion, sources } => {
                    let mut out = [0.0f32; 4];
                    for s in sources {
                        let slot = self.slots.get(s.track as usize).copied();
                        let input = slot.map_or(0.0, |sl| {
                            get(sl, false, pose, rest, animation, frame, controls)[s.component as usize & 3]
                        });
                        for (a, axis) in s.axes.iter().enumerate() {
                            out[a] += axis.eval(input);
                        }
                    }
                    stack.push(if *quaternion { from_euler([out[0], out[1], out[2]]) } else { out });
                }
                Op::Simple(code) => simple(*code, &mut stack, frame),
            }
        }
        stack.len()
    }
}

fn get(
    slot: Slot,
    offset: bool,
    pose: &[Local],
    rest: &[Local],
    animation: &Animation,
    frame: f32,
    controls: &Controls,
) -> [f32; 4] {
    match slot {
        Slot::Bone(b, kind) => {
            let (Some(p), Some(r)) = (pose.get(b), rest.get(b)) else { return [0.0; 4] };
            match (kind, offset) {
                (0, false) => v3(p.translation),
                (0, true) => v3(sub(p.translation, r.translation)),
                (1, false) => p.rotation,
                (1, true) => quat_mul(conjugate(r.rotation), p.rotation),
                (_, false) => v3(p.scale),
                (_, true) => v3(sub(p.scale, r.scale)),
            }
        }
        Slot::Control(key, track, format) => controls.get(&key).copied().unwrap_or_else(|| {
            track.and_then(|t| animation.value(t, frame)).unwrap_or(if format == 1 {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                [0.0; 4]
            })
        }),
    }
}

fn set(slot: Slot, offset: bool, v: [f32; 4], pose: &mut [Local], rest: &[Local], controls: &mut Controls) {
    if v.iter().any(|x| !x.is_finite()) {
        return;
    }
    match slot {
        Slot::Bone(b, kind) => {
            let (Some(p), Some(r)) = (pose.get_mut(b), rest.get(b)) else { return };
            let xyz = [v[0], v[1], v[2]];
            match (kind, offset) {
                (0, false) => p.translation = xyz,
                (0, true) => p.translation = add(r.translation, xyz),
                (1, false) => p.rotation = normalize(v),
                (1, true) => p.rotation = normalize(quat_mul(r.rotation, v)),
                (_, false) => p.scale = xyz,
                (_, true) => p.scale = add(r.scale, xyz),
            }
        }
        Slot::Control(key, ..) => {
            controls.insert(key, v);
        }
    }
}

/// Position and rotation of a bone in the model (parents applied).
fn object_transform(pose: &[Local], parents: &[Option<usize>], bone: usize) -> ([f32; 3], [f32; 4]) {
    let mut chain = vec![bone];
    while let Some(Some(p)) = parents.get(*chain.last().expect("non-empty")) {
        if chain.len() > 256 {
            break;
        }
        chain.push(*p);
    }
    let (mut pos, mut rot) = ([0.0f32; 3], [0.0f32, 0.0, 0.0, 1.0]);
    for b in chain.iter().rev() {
        let Some(l) = pose.get(*b) else { break };
        let t = rotate(rot, [l.translation[0], l.translation[1], l.translation[2]]);
        pos = add(pos, t);
        rot = normalize(quat_mul(rot, l.rotation));
    }
    (pos, rot)
}

fn axis(a: u32) -> [f32; 3] {
    match a {
        0 => [1.0, 0.0, 0.0],
        1 => [0.0, 1.0, 0.0],
        2 => [0.0, 0.0, 1.0],
        3 => [-1.0, 0.0, 0.0],
        4 => [0.0, -1.0, 0.0],
        _ => [0.0, 0.0, -1.0],
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

/// Rotation (in the model) that points the bone's `look_axis` from `own`
/// at `target`, keeping its `up_axis` close to that of `frame`.
fn look_at(target: [f32; 4], frame: [f32; 4], own: [f32; 4], look_axis: u32, up_axis: u32) -> [f32; 4] {
    let Some(f) = unit(sub([target[0], target[1], target[2]], [own[0], own[1], own[2]])) else { return frame };
    let up_hint = rotate(frame, axis(up_axis));
    let Some(side) = unit(cross(f, up_hint)) else { return frame };
    let up = cross(side, f);
    // The basis the bone should have: look axis → f, up axis → up.
    let (la, ua) = (axis(look_axis), axis(up_axis));
    let third_local = cross(la, ua);
    let third = cross(f, up);
    // Rotation matrix mapping local axes (la, ua, third_local) to (f, up, third).
    let m = |l: [f32; 3]| -> [f32; 3] {
        let d = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let (x, y, z) = (d(l, la), d(l, ua), d(l, third_local));
        [f[0] * x + up[0] * y + third[0] * z, f[1] * x + up[1] * y + third[1] * z, f[2] * x + up[2] * y + third[2] * z]
    };
    let (c0, c1, c2) = (m([1.0, 0.0, 0.0]), m([0.0, 1.0, 0.0]), m([0.0, 0.0, 1.0]));
    matrix_to_quat(c0, c1, c2)
}

/// Quaternion of a rotation matrix given by its columns.
fn matrix_to_quat(c0: [f32; 3], c1: [f32; 3], c2: [f32; 3]) -> [f32; 4] {
    let (m00, m11, m22) = (c0[0], c1[1], c2[2]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(c1[2] - c2[1]) / s, (c2[0] - c0[2]) / s, (c0[1] - c1[0]) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (c1[0] + c0[1]) / s, (c2[0] + c0[2]) / s, (c1[2] - c2[1]) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(c1[0] + c0[1]) / s, 0.25 * s, (c2[1] + c1[2]) / s, (c2[0] - c0[2]) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(c2[0] + c0[2]) / s, (c2[1] + c1[2]) / s, 0.25 * s, (c0[1] - c1[0]) / s]
    };
    normalize(q)
}

fn is_rotation(slot: Option<Slot>) -> bool {
    matches!(slot, Some(Slot::Bone(_, 1)) | Some(Slot::Control(_, _, 1)))
}

/// A component as a splatted vector; rotations give Euler angles.
fn component_of(slot: Option<Slot>, v: [f32; 4], c: usize) -> [f32; 4] {
    let v = if is_rotation(slot) { to_euler(v) } else { v };
    [v[c]; 4]
}

fn with_component(slot: Option<Slot>, current: [f32; 4], c: usize, x: f32) -> [f32; 4] {
    if is_rotation(slot) {
        let mut e = to_euler(current);
        e[c] = x;
        from_euler([e[0], e[1], e[2]])
    } else {
        let mut v = current;
        v[c] = x;
        v
    }
}

fn simple(code: u8, stack: &mut Vec<[f32; 4]>, frame: f32) {
    let pop = |stack: &mut Vec<[f32; 4]>| stack.pop().unwrap_or([0.0; 4]);
    let map1 = |a: [f32; 4], f: &dyn Fn(f32) -> f32| a.map(f);
    let zip = |a: [f32; 4], b: [f32; 4], f: &dyn Fn(f32, f32) -> f32| std::array::from_fn(|i| f(a[i], b[i]));
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    match code {
        0x0F | 0x10 | 0x11 | 0x12 | 0x1B | 0x1C | 0x1D | 0x1E | 0x1F | 0x21 | 0x22 => {
            let a = pop(stack);
            stack.push(match code {
                0x0F => map1(a, &f32::abs),
                0x10 => map1(a, &|x| -x),
                0x11 => map1(a, &|x| if x != 0.0 { 1.0 / x } else { 0.0 }),
                0x12 => map1(a, &|x| x.max(0.0).sqrt()),
                0x1B => [-a[0], -a[1], -a[2], a[3]],
                0x1C => map1(a, &|x| x * x),
                0x1D => map1(a, &f32::to_radians),
                0x1E => map1(a, &f32::to_degrees),
                0x1F => map1(a, &|x| x.clamp(0.0, 1.0)),
                0x21 => from_euler([a[0], a[1], a[2]]),
                _ => to_euler(a),
            });
        }
        0x2E..=0x33 | 0x35..=0x38 | 0x40 | 0x48 | 0x49 => {
            let b = pop(stack);
            let a = pop(stack);
            stack.push(match code {
                0x2E => zip(a, b, &|x, y| x + y),
                0x2F => zip(a, b, &|x, y| x - y),
                0x30 => zip(a, b, &|x, y| x * y),
                0x31 => zip(a, b, &f32::min),
                0x32 => zip(a, b, &f32::max),
                0x33 => quat_mul(a, b),
                0x35 => zip(a, b, &|x, y| flag(x > y)),
                0x36 => zip(a, b, &|x, y| flag(x < y)),
                0x37 => zip(a, b, &|x, y| flag(x >= y)),
                0x38 => zip(a, b, &|x, y| flag(x <= y)),
                0x40 => v3(rotate(b, [a[0], a[1], a[2]])),
                0x48 => zip(a, b, &|x, y| flag(x == y)),
                _ => zip(a, b, &|x, y| flag(x != y)),
            });
        }
        0x39..=0x3D => {
            let c = pop(stack);
            let b = pop(stack);
            let a = pop(stack);
            stack.push(match code {
                0x39 => std::array::from_fn(|i| a[i].max(b[i]).min(c[i])),
                // The factor is pushed first.
                0x3A => std::array::from_fn(|i| b[i] + (c[i] - b[i]) * a[i]),
                0x3B => std::array::from_fn(|i| a[i] * b[i] + c[i]),
                0x3C => slerp(a, b, c[0]),
                _ => [a[0], b[0], c[0], 0.0],
            });
        }
        0x3F => stack.push([frame / 30.0; 4]),
        0x46 => stack.push([1.0 / 30.0; 4]),
        _ => {}
    }
}

fn v3(v: [f32; 3]) -> [f32; 4] {
    [v[0], v[1], v[2], 0.0]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}
fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}
fn normalize(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if len > 1e-8 { q.map(|x| x / len) } else { [0.0, 0.0, 0.0, 1.0] }
}
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = quat_mul(quat_mul(q, [v[0], v[1], v[2], 0.0]), conjugate(q));
    [p[0], p[1], p[2]]
}
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot: f32 = (0..4).map(|i| a[i] * b[i]).sum();
    let b = if dot < 0.0 { b.map(|x| -x) } else { b };
    normalize(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t))
}

/// Quaternion from Euler angles (radians), applied X, then Y, then Z.
pub fn from_euler(e: [f32; 3]) -> [f32; 4] {
    let (sx, cx) = (e[0] / 2.0).sin_cos();
    let (sy, cy) = (e[1] / 2.0).sin_cos();
    let (sz, cz) = (e[2] / 2.0).sin_cos();
    quat_mul(quat_mul([0.0, 0.0, sz, cz], [0.0, sy, 0.0, cy]), [sx, 0.0, 0.0, cx])
}

/// Euler angles (radians, X then Y then Z) of a quaternion.
pub fn to_euler(q: [f32; 4]) -> [f32; 4] {
    let [x, y, z, w] = normalize(q);
    let rx = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
    let ry = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin();
    let rz = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
    [rx, ry, rz, 0.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_round_trip() {
        let e = [0.3, -0.2, 0.7];
        let back = to_euler(from_euler(e));
        for i in 0..3 {
            assert!((back[i] - e[i]).abs() < 1e-5, "{back:?}");
        }
    }

    #[test]
    fn stack_arithmetic() {
        let mut s = vec![[1.0, 2.0, 3.0, 4.0], [2.0; 4], [0.5; 4]];
        simple(0x3B, &mut s, 0.0); // 1,2,3,4 * 2 + 0.5
        assert_eq!(s, vec![[2.5, 4.5, 6.5, 8.5]]);
        simple(0x1F, &mut s, 0.0);
        assert_eq!(s, vec![[1.0; 4]]);
    }
}
