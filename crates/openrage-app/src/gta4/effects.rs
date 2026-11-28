//! Particle effects in GTA IV cutscenes: the effect lists (`.wpfl`, see
//! [`rage_formats::wpfl`]) and a small particle system that plays a
//! cutscene's `[EFFECTS]` as camera-facing sprites.
//!
//! The simulation is a pure function of time, so the scene can be scrubbed:
//! every particle is spawned at a fixed time with random values from a hash
//! of its effect, emitter and number, and moved from there to now.

use std::path::Path;
use std::sync::Arc;

use rage_formats::math::{self, Mat4};
use rage_formats::wpfl::{self, EffectList, EmitterRule, Keyframes, ParticleKind, ParticleRule, prop};

/// The effect lists an episode's cutscenes use, the episode's own first.
const LISTS: [&str; 3] =
    ["TLAD/pc/data/effects/gta_ep1.wpfl", "TBoGT/pc/data/effects/gta_ep2.wpfl", "pc/data/effects/gta_core.wpfl"];

/// A decoded texture: width, height and RGBA pixels.
#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The effect lists of an episode with their textures, decoded.
#[derive(Default)]
pub struct Library {
    pub lists: Vec<EffectList>,
    /// Textures by lower-case name (the first list that has one wins).
    textures: std::collections::HashMap<String, Arc<Image>>,
}

impl Library {
    /// Reads the lists for the episode `episode` (`""`, `TLAD/`, `TBoGT/`, as
    /// [`crate::gta4_cutscene::episode`] gives it). Also returns problems
    /// (lists missing or unreadable).
    pub fn load(game: &Path, episode: &str) -> (Self, Vec<String>) {
        let mut lib = Library::default();
        let mut problems = Vec::new();
        let wanted = LISTS.iter().filter(|l| {
            let lower = l.to_ascii_lowercase();
            lower.starts_with("pc/") || (!episode.is_empty() && lower.starts_with(&episode.to_ascii_lowercase()))
        });
        for rel in wanted {
            let read = || -> anyhow::Result<EffectList> {
                let path = rage_rpf::vfs::find_case_insensitive(game, rel)
                    .ok_or_else(|| anyhow::anyhow!("not in the game folder"))?;
                let res = rage_resource::Resource::from_rsc5(&std::fs::read(path)?)?;
                Ok(wpfl::parse(&res)?)
            };
            match read() {
                Ok(list) => {
                    problems.extend(list.errors.iter().map(|e| format!("{rel}: {e}")));
                    lib.add(list);
                }
                Err(e) => problems.push(format!("{rel}: {e:#}")),
            }
        }
        (lib, problems)
    }

    /// Adds a list (searched after those added before).
    pub fn add(&mut self, list: EffectList) {
        if let Some(dict) = &list.textures {
            for t in &dict.textures {
                let key = t.name.to_ascii_lowercase();
                if self.textures.contains_key(&key) {
                    continue;
                }
                if let Some(rgba) = t.to_rgba() {
                    self.textures.insert(key, Arc::new(Image { width: t.width.max(1), height: t.height.max(1), rgba }));
                }
            }
        }
        self.lists.push(list);
    }

    /// The list and effect with this name (`FX_muz_machine` or `muz_machine`).
    pub fn find(&self, name: &str) -> Option<(usize, usize)> {
        let name = wpfl::strip_fx(name);
        self.lists
            .iter()
            .enumerate()
            .find_map(|(l, list)| list.effects.iter().position(|e| e.name.eq_ignore_ascii_case(name)).map(|e| (l, e)))
    }

    pub fn texture(&self, name: &str) -> Option<&Arc<Image>> {
        self.textures.get(&name.to_ascii_lowercase())
    }
}

/// One effect of a cutscene placed in the scene.
#[derive(Debug, Clone)]
pub struct Instance {
    /// Which effect ([`Library::find`]).
    pub list: usize,
    pub effect: usize,
    /// Seconds into the cutscene: when it starts, and until when it keeps
    /// running (the same for a single burst).
    pub start: f32,
    pub end: f32,
    /// Where the effect is (its space: X forward for muzzle flashes), sampled
    /// every [`TRACK_STEP`] seconds from `start`; the last one holds.
    pub track: Vec<Mat4>,
    /// Varies the random values between instances.
    pub seed: u32,
}

/// Seconds between the samples of [`Instance::track`].
pub const TRACK_STEP: f32 = 0.05;

impl Instance {
    /// How long after `start` the effect may still show particles.
    pub fn length(&self, lib: &Library) -> f32 {
        let fx = &lib.lists[self.list].effects[self.effect];
        (self.end - self.start).max(0.0) + fx.duration(&lib.lists[self.list])
    }

    fn at(&self, time: f32) -> Mat4 {
        let i = ((time - self.start).max(0.0) / TRACK_STEP) as usize;
        self.track.get(i.min(self.track.len().saturating_sub(1))).copied().unwrap_or(math::IDENTITY)
    }
}

/// A rotation from Euler angles (radians, about X then Y then Z).
pub fn euler(r: [f32; 3]) -> Mat4 {
    let axis = |a: [f32; 3], angle: f32| {
        let (s, c) = (angle / 2.0).sin_cos();
        math::from_trs([0.0; 3], [a[0] * s, a[1] * s, a[2] * s, c], [1.0; 3])
    };
    let (x, y, z) = (axis([1.0, 0.0, 0.0], r[0]), axis([0.0, 1.0, 0.0], r[1]), axis([0.0, 0.0, 1.0], r[2]));
    math::mul(&z, &math::mul(&y, &x))
}

/// A particle to draw: a square facing the camera.
#[derive(Debug, Clone)]
pub struct Sprite {
    pub position: [f32; 3],
    /// Width and height, metres.
    pub size: [f32; 2],
    /// Turn in the view plane, radians.
    pub rotation: f32,
    /// RGBA, 0 to 1 (alpha not multiplied in).
    pub colour: [f32; 4],
    /// The texture's part to show: u0, v0, u1, v1.
    pub uv: [f32; 4],
    pub texture: Arc<Image>,
    /// Adds light instead of covering.
    pub additive: bool,
}

/// A random number from 0 to 1 for (seed, a, b, c).
fn random(seed: u32, a: u32, b: u32, c: u32) -> f32 {
    let mut h = seed ^ a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B) ^ c.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Bounds (min, max) of a curve's first two values at `t`.
fn range(k: &Keyframes, t: f32, r: f32) -> f32 {
    let v = k.at(t);
    lerp(v[0], v[1], r)
}

/// A random point in a domain at emitter time `t` (in the effect's space).
fn domain_point(d: Option<&wpfl::Domain>, t: f32, r: [f32; 3]) -> [f32; 3] {
    let Some(d) = d else { return [0.0; 3] };
    let p = d.position.at(t);
    let s = d.size.at(t);
    let a = d.rotation.at(t);
    let local = [(r[0] - 0.5) * s[0], (r[1] - 0.5) * s[1], (r[2] - 0.5) * s[2]];
    let turned = math::transform_dir(&euler([a[0].to_radians(), a[1].to_radians(), a[2].to_radians()]), local);
    [p[0] + turned[0], p[1] + turned[1], p[2] + turned[2]]
}

/// Upper bound of particles one emitter run keeps alive, to stay responsive.
const MAX_PER_EMITTER: u32 = 400;
/// Integration step, seconds.
const STEP: f32 = 1.0 / 30.0;

/// The sprites of all effects at `time` (seconds into the cutscene).
pub fn sprites_at(lib: &Library, instances: &[Instance], time: f32) -> Vec<Sprite> {
    let mut out = Vec::new();
    for inst in instances {
        if time < inst.start || time > inst.start + inst.length(lib) {
            continue;
        }
        instance_sprites(lib, inst, time, &mut out);
    }
    out
}

/// Adds the sprites of every sprite event of one effect instance.
fn instance_sprites(lib: &Library, inst: &Instance, time: f32, out: &mut Vec<Sprite>) {
    let list = &lib.lists[inst.list];
    let fx = &list.effects[inst.effect];
    for (e, ev) in fx.events.iter().enumerate() {
        let em = &list.emitters[ev.emitter];
        let pr = &list.particles[ev.particle];
        if pr.kind != ParticleKind::Sprite {
            continue;
        }
        let Some(texture) = pr.texture.as_deref().and_then(|t| lib.texture(t)) else { continue };
        let seed = inst.seed ^ (e as u32).wrapping_mul(0x2545_F491);
        Emission { inst, em, pr, texture, seed, time }.push_sprites(out);
    }
}

/// One sprite event of an effect instance at one point in time.
struct Emission<'a> {
    inst: &'a Instance,
    em: &'a EmitterRule,
    pr: &'a ParticleRule,
    texture: &'a Arc<Image>,
    seed: u32,
    time: f32,
}

/// A particle's start and the random picks of its acceleration and damping.
struct Flight {
    pos: [f32; 3],
    vel: [f32; 3],
    life: f32,
    age: f32,
    acceleration_pick: f32,
    damping_pick: f32,
}

impl Emission<'_> {
    fn push_sprites(&self, out: &mut Vec<Sprite>) {
        for (k, born, run_t) in self.spawns() {
            if born > self.time {
                continue;
            }
            out.extend(self.sprite(k, born, run_t));
        }
    }

    /// Spawn times: one-shot emitters fire every run; others spawn steadily
    /// over the runs. Each is (number, time, emitter life 0..1).
    fn spawns(&self) -> Vec<(u32, f32, f32)> {
        let (inst, em) = (self.inst, self.em);
        let duration = em.duration.max(0.01);
        let window = (inst.end - inst.start).max(0.0);
        let runs = ((window / duration).floor() as u32 + 1).min(1000);
        if em.one_shot { self.one_shot_spawns(runs, duration) } else { self.steady_spawns(runs, duration) }
    }

    fn one_shot_spawns(&self, runs: u32, duration: f32) -> Vec<(u32, f32, f32)> {
        let mut spawns: Vec<(u32, f32, f32)> = Vec::new();
        for run in 0..runs {
            let at = self.inst.start + run as f32 * duration;
            let n = range(&self.em.spawn_rate, 0.0, random(self.seed, run, 0, 1)).round().clamp(0.0, 200.0) as u32;
            let first = spawns.len() as u32;
            spawns.extend((0..n).map(|i| (first + i, at, 0.0)));
        }
        spawns
    }

    fn steady_spawns(&self, runs: u32, duration: f32) -> Vec<(u32, f32, f32)> {
        let (inst, em, time) = (self.inst, self.em, self.time);
        let v = em.spawn_rate.at(0.0);
        let rate = (v[0] + v[1]) / 2.0;
        if rate <= 0.0 {
            return Vec::new();
        }
        let n = (runs as f32 * duration * rate) as u32;
        let life_max = em.life.0.iter().map(|k| k.value[0].max(k.value[1])).fold(0.0, f32::max);
        // Only the particles that can be alive now.
        let from = (((time - life_max - inst.start) * rate).floor().max(0.0) as u32).min(n);
        let to = ((((time - inst.start) * rate).ceil().max(0.0) as u32) + 1).min(n);
        (from.max(to.saturating_sub(MAX_PER_EMITTER))..to)
            .map(|k| {
                let at = inst.start + k as f32 / rate;
                (k, at, ((at - inst.start) % duration) / duration)
            })
            .collect()
    }

    fn rnd(&self, k: u32, channel: u32) -> f32 {
        random(self.seed, k, channel, 7)
    }

    /// The sprite of particle `k`, unless it is dead or invisible.
    fn sprite(&self, k: u32, born: f32, run_t: f32) -> Option<Sprite> {
        let (em, pr) = (self.em, self.pr);
        let r = |c: u32| self.rnd(k, c);
        let life = range(&em.life, run_t, r(0)).max(0.01);
        let age = self.time - born;
        if age > life {
            return None;
        }
        let place = self.inst.at(born);
        let start = math::transform_point(&place, domain_point(em.creation.as_ref(), run_t, [r(1), r(2), r(3)]));
        let vel = math::transform_dir(&place, self.launch_velocity(k, run_t));
        let pos = self.drift(Flight { pos: start, vel, life, age, acceleration_pick: r(8), damping_pick: r(9) });
        let f = age / life;
        let size = self.size_at(f, run_t, r(10));
        let colour = self.colour_at(f, k);
        if colour[3] <= 0.002 || size[0] <= 0.0 || size[1] <= 0.0 {
            return None;
        }
        let rotation = range(pr.property(prop::ROTATION), 0.0, r(13)).to_radians()
            + range(pr.property(prop::SPIN), f, r(14)) * age;
        Some(Sprite {
            position: pos,
            size,
            rotation,
            colour,
            uv: frame_uv(&pr.frames, pr.property(prop::FRAME_RATE), age, f, r(15), r(16)),
            texture: Arc::clone(self.texture),
            additive: pr.additive,
        })
    }

    /// Towards a point of the target domain, as fast as the speed says.
    fn launch_velocity(&self, k: u32, run_t: f32) -> [f32; 3] {
        let em = self.em;
        let r = |c: u32| self.rnd(k, c);
        let speed = range(&em.speed, run_t, r(4));
        let from = domain_point(em.creation.as_ref(), run_t, [0.5; 3]);
        let to = domain_point(em.target.as_ref(), run_t, [r(5), r(6), r(7)]);
        let dir = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len > 1e-6 { dir.map(|c| c / len * speed) } else { [0.0; 3] }
    }

    /// Moves from birth to now.
    fn drift(&self, flight: Flight) -> [f32; 3] {
        let Flight { mut pos, mut vel, life, age, acceleration_pick: ra, damping_pick: rd } = flight;
        let pr = self.pr;
        let (amin, amax) = (pr.property(prop::ACCELERATION_MIN), pr.property(prop::ACCELERATION_MAX));
        let (dmin, dmax) = (pr.property(prop::DAMPING_MIN), pr.property(prop::DAMPING_MAX));
        let mut t = 0.0;
        while t < age {
            let dt = STEP.min(age - t);
            let f = t / life;
            let (a0, a1, d0, d1) = (amin.at(f), amax.at(f), dmin.at(f), dmax.at(f));
            for c in 0..3 {
                let acc = lerp(a0[c], a1[c], ra);
                let damp = lerp(d0[c], d1[c], rd).max(0.0);
                vel[c] += (acc - vel[c] * damp) * dt;
                pos[c] += vel[c] * dt;
            }
            t += dt;
        }
        pos
    }

    fn size_at(&self, f: f32, run_t: f32, rs: f32) -> [f32; 2] {
        let size = self.pr.property(prop::SIZE).at(f);
        let scale = self.em.size_scale.at(run_t)[0] / 100.0;
        let scale = if scale > 0.0 { scale } else { 1.0 };
        [lerp(size[0], size[2], rs) * scale, lerp(size[1], size[3], rs) * scale]
    }

    fn colour_at(&self, f: f32, k: u32) -> [f32; 4] {
        let pr = self.pr;
        let (c0, c1) = (pr.property(prop::COLOUR_MIN).at(f), pr.property(prop::COLOUR_MAX).at(f));
        let rc = self.rnd(k, 11);
        let mut colour: [f32; 4] = std::array::from_fn(|c| lerp(c0[c], c1[c], rc).clamp(0.0, 1.0));
        let alpha = pr.property(prop::ALPHA);
        if !alpha.0.is_empty() {
            colour[3] *= range(alpha, f, self.rnd(k, 12)).clamp(0.0, 1.0);
        }
        colour
    }
}

/// The part of the texture sheet a particle shows at `age`.
fn frame_uv(frames: &wpfl::Frames, rate: &Keyframes, age: f32, f: f32, r_start: f32, r_rate: f32) -> [f32; 4] {
    let (cols, rows) = (frames.columns.max(1), frames.rows.max(1));
    let total = cols * rows;
    let first = ((r_start * (frames.start_max + 1) as f32) as u32).min(frames.start_max).min(total - 1);
    let frame = if frames.last > first {
        let fps = range(rate, f, r_rate).max(0.0);
        (first + (age * fps) as u32).min(frames.last.min(total - 1))
    } else {
        first
    };
    let (c, r) = (frame % cols, frame / cols);
    let (w, h) = (1.0 / cols as f32, 1.0 / rows as f32);
    [c as f32 * w, r as f32 * h, (c + 1) as f32 * w, (r + 1) as f32 * h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rage_formats::wpfl::{EffectEvent, EffectRule, EmitterRule, Frames, Key, ParticleRule};

    fn curve(v: [f32; 4]) -> Keyframes {
        Keyframes(vec![Key { time: 0.0, value: v }])
    }

    /// One effect: a one-shot emitter of `count` particles moving +X at 1 m/s
    /// (towards a target 3 m ahead).
    fn library(count: f32) -> Library {
        let list = EffectList {
            effects: vec![EffectRule {
                name: "muz_test".into(),
                events: vec![EffectEvent { emitter: 0, particle: 0, playback_rate: [1.0; 2], zoom: [1.0; 2] }],
            }],
            emitters: vec![EmitterRule {
                duration: 0.01,
                one_shot: true,
                spawn_rate: curve([count, count, 0.0, 0.0]),
                life: curve([1.0, 1.0, 0.0, 0.0]),
                speed: curve([1.0, 1.0, 0.0, 0.0]),
                size_scale: curve([100.0; 4]),
                target: Some(wpfl::Domain { position: curve([3.0, 0.0, 0.0, 0.0]), ..Default::default() }),
                ..Default::default()
            }],
            particles: vec![ParticleRule {
                texture: Some("smoke".into()),
                frames: Frames { start_max: 0, last: 3, columns: 2, rows: 2 },
                properties: vec![
                    (prop::COLOUR_MIN, curve([1.0; 4])),
                    (prop::COLOUR_MAX, curve([1.0; 4])),
                    (prop::SIZE, curve([0.5, 0.5, 0.5, 0.5])),
                    (prop::FRAME_RATE, curve([2.0, 2.0, 0.0, 0.0])),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut lib = Library::default();
        lib.textures.insert("smoke".into(), Arc::new(Image { width: 1, height: 1, rgba: vec![255; 4] }));
        lib.lists.push(list);
        lib
    }

    #[test]
    fn particles_fly_and_die() {
        let lib = library(3.0);
        assert_eq!(lib.find("FX_MUZ_TEST"), Some((0, 0)));
        let inst = Instance {
            list: 0,
            effect: 0,
            start: 10.0,
            end: 10.0,
            track: vec![math::from_trs([5.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3])],
            seed: 1,
        };
        assert!(sprites_at(&lib, std::slice::from_ref(&inst), 9.9).is_empty());
        let now = sprites_at(&lib, std::slice::from_ref(&inst), 10.5);
        assert_eq!(now.len(), 3);
        for s in &now {
            assert!((s.position[0] - 5.5).abs() < 1e-4, "{:?}", s.position);
            assert_eq!(s.size, [0.5, 0.5]);
            // Frame 1 of a 2×2 sheet after half a second at 2 frames a second.
            assert_eq!(s.uv, [0.5, 0.0, 1.0, 0.5]);
        }
        assert!(sprites_at(&lib, &[inst], 11.2).is_empty());
    }

    #[test]
    fn euler_turns_about_z_last() {
        let m = euler([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        let v = math::transform_dir(&m, [1.0, 0.0, 0.0]);
        assert!(v[0].abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6);
    }
}
