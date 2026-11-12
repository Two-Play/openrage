//! Posing a skeleton with an animation, for skinning.

use rage_formats::clip::{Animation, TRACK_BONE_ROTATION, TRACK_BONE_SCALE, TRACK_BONE_TRANSLATION};
use rage_formats::drawable::Skeleton;
use rage_formats::math::{self, Mat4};

use crate::expr::Local;

/// Which animation tracks drive which skeleton bones.
pub struct Binding {
    /// Per bone: translation, rotation and scale track indices.
    tracks: Vec<[Option<usize>; 3]>,
    /// Inverse bind pose per bone.
    inverse_bind: Vec<Mat4>,
    /// The ped's expressions bound to this skeleton and animation.
    programs: Vec<crate::expr::Program>,
    parents: Vec<Option<usize>>,
}

impl Binding {
    pub fn new(skeleton: &Skeleton, animation: &Animation) -> Self {
        let mut tracks = vec![[None; 3]; skeleton.bones.len()];
        for (i, t) in animation.tracks.iter().enumerate() {
            let Some(b) = skeleton.find_tag(t.bone) else { continue };
            let slot = match t.track {
                TRACK_BONE_TRANSLATION => 0,
                TRACK_BONE_ROTATION => 1,
                TRACK_BONE_SCALE => 2,
                _ => continue,
            };
            tracks[b][slot] = Some(i);
        }
        let inverse_bind = skeleton.world_transforms().iter().map(math::inverse_affine).collect();
        let parents =
            skeleton.bones.iter().enumerate().map(|(i, b)| usize::try_from(b.parent).ok().filter(|&p| p < i)).collect();
        Self { tracks, inverse_bind, programs: Vec::new(), parents }
    }

    /// Also runs the ped's expressions each frame (face and helper bones).
    pub fn with_expressions(
        mut self,
        skeleton: &Skeleton,
        animation: &Animation,
        expressions: &[rage_formats::expression::Expression],
    ) -> Self {
        self.programs = expressions.iter().map(|e| crate::expr::Program::new(e, skeleton, animation)).collect();
        self
    }

    /// How many bones the animation moves.
    pub fn animated_bones(&self) -> usize {
        self.tracks.iter().filter(|t| t.iter().any(Option::is_some)).count()
    }

    /// Skinning matrices (posed world × inverse bind) at a fractional frame.
    pub fn skin(&self, skeleton: &Skeleton, animation: &Animation, frame: f32) -> Vec<Mat4> {
        let rest: Vec<Local> = skeleton
            .bones
            .iter()
            .map(|b| Local { translation: b.translation, rotation: b.rotation, scale: b.scale })
            .collect();
        let mut pose: Vec<Local> = rest
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let [t, rot, s] = self.tracks.get(i).copied().unwrap_or_default();
                let value = |track: Option<usize>| track.and_then(|k| animation.value(k, frame));
                Local {
                    translation: value(t).map_or(r.translation, |v| [v[0], v[1], v[2]]),
                    rotation: value(rot).unwrap_or(r.rotation),
                    scale: value(s).map_or(r.scale, |v| [v[0], v[1], v[2]]),
                }
            })
            .collect();
        if !self.programs.is_empty() {
            let mut controls = crate::expr::Controls::new();
            for p in &self.programs {
                p.run(&mut pose, &rest, &self.parents, animation, frame, &mut controls);
            }
        }
        let mut world: Vec<Mat4> = Vec::with_capacity(skeleton.bones.len());
        for (i, (bone, l)) in skeleton.bones.iter().zip(&pose).enumerate() {
            let local = math::from_trs(l.translation, l.rotation, l.scale);
            let w = match usize::try_from(bone.parent).ok().filter(|&p| p < i) {
                Some(p) => math::mul(&world[p], &local),
                None => local,
            };
            world.push(w);
        }
        world.iter().zip(&self.inverse_bind).map(|(w, ib)| math::mul(w, ib)).collect()
    }
}

/// Applies skinning to a vertex (position or direction with `w = 0`).
pub fn skin_point(skin: &[Mat4], indices: [u16; 4], weights: [f32; 4], p: [f32; 3], direction: bool) -> [f32; 3] {
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return p;
    }
    let mut out = [0.0f32; 3];
    for k in 0..4 {
        if weights[k] <= 0.0 {
            continue;
        }
        let Some(m) = skin.get(indices[k] as usize) else { continue };
        let v = if direction { math::transform_dir(m, p) } else { math::transform_point(m, p) };
        for c in 0..3 {
            out[c] += v[c] * weights[k] / total;
        }
    }
    out
}
