//! Posing a skeleton with an animation, for skinning.

use rage_formats::clip::{Animation, TRACK_BONE_ROTATION, TRACK_BONE_SCALE, TRACK_BONE_TRANSLATION};
use rage_formats::drawable::Skeleton;
use rage_formats::math::{self, Mat4};

use crate::expr::Local;

/// Reads a clip dictionary (`.ycd`, GTA V or RDR2; GTA IV `.wad`) or a cutscene's
/// animation (`.cut`, its sections joined), mods included.
pub fn load_clips(
    game_dir: &std::path::Path,
    keys: Option<std::sync::Arc<rage_crypto::GtaVKeys>>,
    vpath: &str,
) -> anyhow::Result<rage_formats::clip::ClipDictionary> {
    use rage_rpf::vfs::Node;
    match rage_mods::resolve(game_dir, vpath, keys)?.0 {
        Node::ArchiveFile(rpf, id) if vpath.to_ascii_lowercase().ends_with(".cut") => {
            crate::cutscene::animation(&rpf, id)
        }
        Node::ArchiveFile(rpf, id) => Ok(rage_formats::clip::parse_ycd(&rpf.read_resource(id)?)?),
        Node::Gta4File(archive, id) => Ok(rage_formats::wad::parse_wad(&archive.read_resource(id)?)?),
        Node::Rpf8File(rpf, i) => {
            let entry =
                rpf.entries().and_then(|e| e.get(i)).ok_or_else(|| anyhow::anyhow!("archive table not readable"))?;
            Ok(rage_formats::clip::parse_ycd(&rpf.read_resource(entry)?)?)
        }
        _ => anyhow::bail!("{vpath} is not a file inside an archive"),
    }
}

/// GTA IV's twist and roll helpers (`Char_L_ForeTwist`, `L_ArmRoll`, …) are
/// moved by the game, not by animations: each rests where an animated sibling
/// bone rests (the forearm, the upper arm). Without a track of their own they
/// turn with that sibling, so skin weighted to them follows the limb. Only
/// for GTA IV skeletons (`Char_…` bones); GTA V's helpers have expressions.
///
/// A twist or roll helper turned against its sibling at rest (the forearm
/// twist, about 20° about the arm) keeps that turn: the returned offset per
/// bone (sibling's rest⁻¹ × own rest) follows the sibling's animated rotation.
fn follow_twist_bones(skeleton: &Skeleton, tracks: &mut [[Option<usize>; 3]]) -> Vec<Option<[f32; 4]>> {
    let mut offsets = vec![None; tracks.len()];
    if skeleton.find_name("Char_Pelvis").is_none() {
        return offsets;
    }
    let bones = &skeleton.bones;
    for (i, b) in bones.iter().enumerate() {
        if tracks[i][1].is_some() {
            continue;
        }
        let lower = b.name.to_ascii_lowercase();
        let helper = lower.contains("twist") || lower.contains("roll");
        let near = |o: &rage_formats::drawable::Bone| {
            let dt: f32 = (0..3).map(|k| (b.translation[k] - o.translation[k]).powi(2)).sum();
            let dot: f32 = (0..4).map(|k| b.rotation[k] * o.rotation[k]).sum();
            dt < 0.03 * 0.03 && (helper || dot.abs() > 0.999)
        };
        let sibling = (0..bones.len())
            .find(|&j| j != i && bones[j].parent == b.parent && tracks[j][1].is_some() && near(&bones[j]));
        if let Some(j) = sibling {
            tracks[i][1] = tracks[j][1];
            let r = bones[j].rotation;
            let offset = crate::expr::quat_mul([-r[0], -r[1], -r[2], r[3]], b.rotation);
            // Only worth applying when the rests differ.
            if offset[3].abs() < 0.9999 {
                offsets[i] = Some(offset);
            }
        }
    }
    offsets
}

/// Which animation tracks drive which skeleton bones.
pub struct Binding {
    /// Per bone: translation, rotation and scale track indices.
    tracks: Vec<[Option<usize>; 3]>,
    /// Per bone: a turn after the animated rotation (helpers following a
    /// sibling, see [`follow_twist_bones`]).
    rotation_offsets: Vec<Option<[f32; 4]>>,
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
        let rotation_offsets = follow_twist_bones(skeleton, &mut tracks);
        let inverse_bind = skeleton.world_transforms().iter().map(math::inverse_affine).collect();
        let parents =
            skeleton.bones.iter().enumerate().map(|(i, b)| usize::try_from(b.parent).ok().filter(|&p| p < i)).collect();
        Self { tracks, rotation_offsets, inverse_bind, programs: Vec::new(), parents }
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
        self.skin_with(skeleton, animation, frame, crate::expr::Controls::new())
    }

    /// Like [`Binding::skin`], with control values the expressions start
    /// from (RDR2 face shapes).
    pub fn skin_with(
        &self,
        skeleton: &Skeleton,
        animation: &Animation,
        frame: f32,
        mut controls: crate::expr::Controls,
    ) -> Vec<Mat4> {
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
                    rotation: match (value(rot), self.rotation_offsets.get(i).copied().flatten()) {
                        (Some(q), Some(offset)) => crate::expr::quat_mul(q, offset),
                        (Some(q), None) => q,
                        (None, _) => r.rotation,
                    },
                    scale: value(s).map_or(r.scale, |v| [v[0], v[1], v[2]]),
                }
            })
            .collect();
        if !self.programs.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rage_formats::clip::BoneTrack;
    use rage_formats::drawable::Bone;

    #[test]
    fn twist_helpers_keep_their_turn() {
        let bone = |name: &str, tag, parent, translation, rotation| Bone {
            name: name.into(),
            tag,
            parent,
            translation,
            rotation,
            scale: [1.0; 3],
        };
        // The twist rests on the forearm, turned 40° about the arm (X).
        let (s, c) = (20f32.to_radians().sin(), 20f32.to_radians().cos());
        let skeleton = Skeleton {
            bones: vec![
                bone("Char_Pelvis", 0, -1, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                bone("Char_R_Forearm", 1, 0, [0.3, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
                bone("Char_R_ForeTwist", 2, 0, [0.3, 0.0, 0.0], [s, 0.0, 0.0, c]),
            ],
        };
        // The forearm turns 90° about Z.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let animation = Animation::from_frames(
            1,
            0.0,
            vec![BoneTrack { bone: 1, format: 1, track: TRACK_BONE_ROTATION }],
            vec![vec![[0.0, 0.0, h, h]]],
        );
        let binding = Binding::new(&skeleton, &animation);
        let skin = binding.skin(&skeleton, &animation, 0.0);
        // The twist's skin matrix moves its vertices as the forearm's do.
        let p = [0.6, 0.1, 0.0];
        let (a, b) = (math::transform_point(&skin[1], p), math::transform_point(&skin[2], p));
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-5, "{a:?} {b:?}");
        }
    }
}
