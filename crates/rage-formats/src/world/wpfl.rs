//! GTA IV particle effect lists (`.wpfl`, `rage::ptxFxList` of the old
//! `rmptfx` particle system), RSC5 resources with 32-bit pointers.
//!
//! No public description of the format exists; this layout was worked out
//! from the game's own files (`gta_core.wpfl`, `gta_ep1.wpfl`,
//! `gta_ep2.wpfl`, resource type 36). The class pointers (vtables) differ
//! between the base game and the episodes, so nothing here relies on them.
//!
//! * root: `0x00` texture dictionary (the usual GTA IV one), `0x08` model
//!   dictionary, `0x0C` particle rules, `0x14` emitter rules, `0x18` effect
//!   rules. Each rule dictionary keeps the joaat hashes of the names at
//!   `0x10` (u16 count at `0x14`) and pointers at `0x18` (count at `0x1C`).
//! * keyframe property (`0x28` bytes in rules, `0x34` in particle rules):
//!   keys at `0x18` (u16 count at `0x1C`); a key is `0x30` bytes: time
//!   (0 to 1 of the owner's life) at `0x00`, value (vec4) at `0x10`, slope at
//!   `0x20`. Particle-rule properties carry their id at `0x28`.
//! * effect rule: name at `0xF8`, its events at `0x130` (u16 count at
//!   `0x134`). An emitter event names its emitter and particle rule at
//!   `0x40`/`0x44` and points to them at `0x48`/`0x4C`; `0x20..0x38` hold
//!   three (min, max) pairs: playback rate, zoom and a third scale.
//! * emitter rule: `0x08` duration (seconds), `0x14` creation domain (where
//!   particles start), `0x1C` target domain (where they head: their
//!   direction, scaled by the speed property), ten
//!   properties from `0x24` (see [`EmitterRule`]), name at `0x1E0`, one-shot
//!   flag (byte) at `0x1EC`.
//! * domain: `0x08` shape, four properties from `0x10`: position, rotation
//!   (degrees), size, inner size.
//! * particle rule: name at `0x124`, its kind inline at `0x128`
//!   (`ptxsprite` or `ptxmodel`). Sprites: blend mode at `0x104` (7 adds
//!   light), texture frames at `0x174..0x184` (highest start frame, last
//!   frame, columns, rows), 24 properties from `0x198` (see
//!   [`ParticleRule`]), the shader at `0x678` (name `0x04`, technique `0x08`,
//!   variables at `0x14` with a u16 count at `0x18`; the texture variable has
//!   `6` in the top byte of `0x10` and the texture's name at `0x24`).

use rage_resource::{Resource, ResourceError};

use crate::ytd::{self, TextureDictionary};

mod read;
use read::{RuleIndex, read_effects, read_models, read_rules, read_textures};

/// Resource type of the effect lists cutscenes use (`gta_core`, the episodes').
pub const TYPE: u32 = 36;

const ROOT: u64 = Resource::ROOT;
const MAX_ENTRIES: u64 = 8192;
const MAX_KEYS: u64 = 256;
const MAX_EVENTS: u64 = 256;

/// Particle-rule property ids (see [`ParticleRule::property`]).
pub mod prop {
    /// Colour, lower and upper bound (RGBA, 0 to 1).
    pub const COLOUR_MIN: u32 = 0x00;
    pub const COLOUR_MAX: u32 = 0x01;
    /// Size: (smallest width, smallest height, largest width, largest height), metres.
    pub const SIZE: u32 = 0x04;
    /// Starting rotation, (min, max) degrees.
    pub const ROTATION: u32 = 0x06;
    /// Spin, (min, max) radians per second.
    pub const SPIN: u32 = 0x08;
    /// Acceleration, lower and upper bound (xyz, m/s²).
    pub const ACCELERATION_MIN: u32 = 0x09;
    pub const ACCELERATION_MAX: u32 = 0x0A;
    /// Damping of the velocity, lower and upper bound (xyz, per second).
    pub const DAMPING_MIN: u32 = 0x0B;
    pub const DAMPING_MAX: u32 = 0x0C;
    /// Texture frames per second, (min, max).
    pub const FRAME_RATE: u32 = 0x10;
    /// Opacity, (min, max).
    pub const ALPHA: u32 = 0x13;
}

/// One key of a curve: a time (0 to 1 of its owner's life) and a value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub time: f32,
    pub value: [f32; 4],
}

/// A curve of values over its owner's life (an emitter's or a particle's).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Keyframes(pub Vec<Key>);

impl Keyframes {
    /// The value at `t` (0 to 1), linear between keys and held before the
    /// first and after the last; zero without keys.
    pub fn at(&self, t: f32) -> [f32; 4] {
        let keys = &self.0;
        let Some(first) = keys.first() else { return [0.0; 4] };
        if t <= first.time || keys.len() == 1 {
            return first.value;
        }
        for pair in keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.time {
                let span = b.time - a.time;
                let f = if span > 1e-6 { (t - a.time) / span } else { 1.0 };
                return std::array::from_fn(|k| a.value[k] + (b.value[k] - a.value[k]) * f);
            }
        }
        keys[keys.len() - 1].value
    }
}

/// Where particles start (creation domain) or where they head (target
/// domain): a shape placed and turned in the effect's space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Domain {
    pub shape: u32,
    pub position: Keyframes,
    /// Degrees about X, Y and Z.
    pub rotation: Keyframes,
    pub size: Keyframes,
    pub inner_size: Keyframes,
}

/// How particles are spawned. The curves run over the emitter's duration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmitterRule {
    pub name: String,
    /// Seconds one run of the emitter lasts.
    pub duration: f32,
    /// All particles at once (`spawn_rate` is then their number).
    pub one_shot: bool,
    pub creation: Option<Domain>,
    pub target: Option<Domain>,
    /// Particles per second (or in all, see `one_shot`), (min, max).
    pub spawn_rate: Keyframes,
    /// Particles per metre the emitter moves, (min, max).
    pub spawn_per_metre: Keyframes,
    pub playback_rate: Keyframes,
    /// Seconds a particle lives, (min, max).
    pub life: Keyframes,
    /// Starting speed, m/s (min, max), towards the target domain.
    pub speed: Keyframes,
    /// Scales the particles' size, percent (x, y, z).
    pub size_scale: Keyframes,
    /// The other four properties (percent scales and one unused).
    pub others: Vec<Keyframes>,
}

/// What a particle is drawn as.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParticleKind {
    /// A camera-facing textured square.
    #[default]
    Sprite,
    /// A model from the list's model dictionary (debris, muzzle flash prongs).
    Model,
}

/// A sprite texture used as a sheet of frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Frames {
    /// A particle starts on a frame from 0 to this.
    pub start_max: u32,
    /// The frame an animation runs to (0: no animation).
    pub last: u32,
    pub columns: u32,
    pub rows: u32,
}

/// How single particles look and behave. The curves run over a particle's life.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParticleRule {
    pub name: String,
    pub kind: ParticleKind,
    /// The texture's name (sprites), found in this list's textures or another's.
    pub texture: Option<String>,
    pub shader: String,
    pub technique: String,
    /// Light adds up (glows, flashes) instead of covering what is behind.
    pub additive: bool,
    pub frames: Frames,
    /// Sprites: keyframe properties by id (see [`prop`]).
    pub properties: Vec<(u32, Keyframes)>,
}

impl ParticleRule {
    /// The property with this id (see [`prop`]); an empty curve when absent.
    pub fn property(&self, id: u32) -> &Keyframes {
        static EMPTY: Keyframes = Keyframes(Vec::new());
        self.properties.iter().find(|(i, _)| *i == id).map_or(&EMPTY, |(_, k)| k)
    }
}

/// One emitter of an effect, with the particles it spawns.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectEvent {
    /// Index into [`EffectList::emitters`].
    pub emitter: usize,
    /// Index into [`EffectList::particles`].
    pub particle: usize,
    pub playback_rate: [f32; 2],
    pub zoom: [f32; 2],
}

/// An effect as the game and cutscenes name it (`muz_machine`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectRule {
    pub name: String,
    pub events: Vec<EffectEvent>,
}

impl EffectRule {
    /// How long one run lasts: the longest emitter plus its particles' life.
    pub fn duration(&self, list: &EffectList) -> f32 {
        self.events
            .iter()
            .map(|e| {
                let em = &list.emitters[e.emitter];
                let life = em.life.0.iter().map(|k| k.value[0].max(k.value[1])).fold(0.0, f32::max);
                em.duration + life
            })
            .fold(0.0, f32::max)
    }
}

/// A particle effect list.
#[derive(Debug, Clone, Default)]
pub struct EffectList {
    pub effects: Vec<EffectRule>,
    pub emitters: Vec<EmitterRule>,
    pub particles: Vec<ParticleRule>,
    pub textures: Option<TextureDictionary>,
    /// Models particles use, by name hash.
    pub models: usize,
    /// Events that are not emitters (they start other effects), left out.
    pub other_events: usize,
    /// Parts that could not be read.
    pub errors: Vec<String>,
}

impl EffectList {
    /// The effect with this name (case does not matter; a leading `FX_`, as
    /// cutscenes write it, is ignored).
    pub fn effect(&self, name: &str) -> Option<&EffectRule> {
        let name = strip_fx(name);
        self.effects.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }
}

/// An effect name without the `FX_` cutscenes put in front.
pub fn strip_fx(name: &str) -> &str {
    match name.get(..3) {
        Some(p) if p.eq_ignore_ascii_case("fx_") => &name[3..],
        _ => name,
    }
}

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

fn ptr(res: &Resource, at: u64) -> Result<u64, ResourceError> {
    res.u32(at).map(u64::from)
}

fn f32_at(res: &Resource, at: u64) -> Result<f32, ResourceError> {
    res.u32(at).map(f32::from_bits)
}

fn string_at(res: &Resource, at: u64) -> Result<String, ResourceError> {
    match ptr(res, at)? {
        0 => Ok(String::new()),
        p => res.string(p),
    }
}

fn keyframes(res: &Resource, at: u64) -> Result<Keyframes, ResourceError> {
    let list = ptr(res, at + 0x18)?;
    let count = res.u16(at + 0x1C)? as u64;
    if count > MAX_KEYS {
        return Err(bad(format!("implausible curve of {count} keys")));
    }
    if list == 0 {
        return Ok(Keyframes::default());
    }
    let keys = (0..count)
        .map(|i| {
            let k = list + i * 0x30;
            Ok(Key {
                time: f32_at(res, k)?,
                value: [f32_at(res, k + 0x10)?, f32_at(res, k + 0x14)?, f32_at(res, k + 0x18)?, f32_at(res, k + 0x1C)?],
            })
        })
        .collect::<Result<_, ResourceError>>()?;
    Ok(Keyframes(keys))
}

/// A rule dictionary's entries (pointers).
fn dictionary(res: &Resource, at: u64) -> Result<Vec<u64>, ResourceError> {
    if at == 0 {
        return Ok(Vec::new());
    }
    let list = ptr(res, at + 0x18)?;
    let count = res.u16(at + 0x1C)? as u64;
    if count > MAX_ENTRIES {
        return Err(bad(format!("implausible dictionary of {count} entries")));
    }
    (0..count).map(|i| ptr(res, list + i * 4)).collect()
}

fn domain(res: &Resource, at: u64) -> Result<Option<Domain>, ResourceError> {
    if at == 0 {
        return Ok(None);
    }
    let p = |k: u64| keyframes(res, at + 0x10 + k * 0x28);
    Ok(Some(Domain { shape: res.u32(at + 0x08)?, position: p(0)?, rotation: p(1)?, size: p(2)?, inner_size: p(3)? }))
}

fn emitter(res: &Resource, at: u64) -> Result<EmitterRule, ResourceError> {
    let mut props = (0..10).map(|k| keyframes(res, at + 0x24 + k * 0x28)).collect::<Result<Vec<_>, _>>()?;
    let mut take = |i: usize| std::mem::take(&mut props[i]);
    let (spawn_rate, spawn_per_metre, playback_rate, life, speed, size_scale) =
        (take(0), take(1), take(2), take(3), take(4), take(5));
    Ok(EmitterRule {
        name: string_at(res, at + 0x1E0)?,
        duration: f32_at(res, at + 0x08)?,
        one_shot: res.u8(at + 0x1EC)? & 1 != 0,
        creation: domain(res, ptr(res, at + 0x14)?)?,
        target: domain(res, ptr(res, at + 0x1C)?)?,
        spawn_rate,
        spawn_per_metre,
        playback_rate,
        life,
        speed,
        size_scale,
        others: props.split_off(6),
    })
}

/// The particle kind name, lower case.
fn kind_name(res: &Resource, at: u64) -> Result<String, ResourceError> {
    let kind_bytes = res.bytes(at + 0x128, 16)?;
    Ok(kind_bytes.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>().to_ascii_lowercase())
}

/// The texture name a shader variable points at; `None` for other variables.
fn texture_variable(res: &Resource, v: u64) -> Result<Option<String>, ResourceError> {
    if v != 0 && res.u32(v + 0x10)? >> 24 == 6 && ptr(res, v + 0x24)? != 0 {
        let name = string_at(res, v + 0x24)?;
        return Ok(Some(ytd::gta4_texture_name(&name).to_owned()));
    }
    Ok(None)
}

/// A sprite's colour texture: the first of the shader's texture variables.
fn sprite_texture(res: &Resource, shader: u64) -> Result<Option<String>, ResourceError> {
    let vars = ptr(res, shader + 0x14)?;
    let count = res.u16(shader + 0x18)? as u64;
    let mut texture = None;
    for i in 0..count.min(64) {
        let name = texture_variable(res, ptr(res, vars + i * 4)?)?;
        // The colour texture comes first; a normal map may follow.
        if texture.is_none() {
            texture = name;
        }
    }
    Ok(texture)
}

fn sprite_frames(res: &Resource, at: u64) -> Result<Frames, ResourceError> {
    Ok(Frames {
        start_max: res.u32(at + 0x174)?,
        last: res.u32(at + 0x178)?,
        columns: res.u32(at + 0x17C)?.clamp(1, 64),
        rows: res.u32(at + 0x180)?.clamp(1, 64),
    })
}

/// The fields only sprites have.
fn read_sprite(res: &Resource, at: u64, rule: &mut ParticleRule) -> Result<(), ResourceError> {
    rule.additive = res.u32(at + 0x104)? == 7;
    rule.frames = sprite_frames(res, at)?;
    for k in 0..24 {
        let p = at + 0x198 + k * 0x34;
        rule.properties.push((res.u32(p + 0x28)?, keyframes(res, p)?));
    }
    let shader = at + 0x678;
    rule.shader = string_at(res, shader + 0x04)?;
    rule.technique = string_at(res, shader + 0x08)?;
    rule.texture = sprite_texture(res, shader)?;
    Ok(())
}

fn particle(res: &Resource, at: u64) -> Result<ParticleRule, ResourceError> {
    let kind_name = kind_name(res, at)?;
    let mut rule = ParticleRule { name: string_at(res, at + 0x124)?, ..Default::default() };
    match kind_name.as_str() {
        "ptxsprite" => {}
        "ptxmodel" => {
            rule.kind = ParticleKind::Model;
            return Ok(rule);
        }
        other => return Err(bad(format!("{}: unknown particle kind {other:?}", rule.name))),
    }
    read_sprite(res, at, &mut rule)?;
    Ok(rule)
}

/// Reads an effect list (resource type [`TYPE`]).
pub fn parse(res: &Resource) -> Result<EffectList, ResourceError> {
    if res.version != TYPE {
        return Err(bad(format!(
            "effect lists of resource type {} are not supported (only {TYPE}, the one cutscenes use)",
            res.version
        )));
    }
    let mut out = EffectList::default();
    read_textures(res, &mut out)?;
    read_models(res, &mut out)?;
    let particles =
        read_rules(res, ptr(res, ROOT + 0x0C)?, particle, "particle rule", (&mut out.particles, &mut out.errors))?;
    let emitters =
        read_rules(res, ptr(res, ROOT + 0x14)?, emitter, "emitter rule", (&mut out.emitters, &mut out.errors))?;
    let index = RuleIndex { emitters, particles };
    read_effects(res, &index, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built system segment: `put` places little-endian words.
    struct Mem(Vec<u8>);

    const BASE: u64 = 0x5000_0000;

    impl Mem {
        fn new() -> Self {
            Mem(vec![0; 0x4000])
        }
        fn u32(&mut self, at: u64, v: u32) {
            self.0[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        }
        fn f32(&mut self, at: u64, v: f32) {
            self.u32(at, v.to_bits());
        }
        fn ptr(&mut self, at: u64, to: u64) {
            self.u32(at, (BASE + to) as u32);
        }
        fn bytes(&mut self, at: u64, b: &[u8]) {
            self.0[at as usize..at as usize + b.len()].copy_from_slice(b);
        }
        /// A curve at `at` (property header) with its keys at `keys`.
        fn curve(&mut self, at: u64, keys_at: u64, keys: &[(f32, [f32; 4])]) {
            self.ptr(at + 0x18, keys_at);
            self.u32(at + 0x1C, keys.len() as u32 | (keys.len() as u32) << 16);
            for (i, (t, v)) in keys.iter().enumerate() {
                let k = keys_at + i as u64 * 0x30;
                self.f32(k, *t);
                for (c, x) in v.iter().enumerate() {
                    self.f32(k + 0x10 + c as u64 * 4, *x);
                }
            }
        }
        /// A dictionary at `at` with one entry pointing at `to`.
        fn dict(&mut self, at: u64, list_at: u64, to: u64) {
            self.ptr(at + 0x18, list_at);
            self.u32(at + 0x1C, 0x0001_0001);
            self.ptr(list_at, to);
        }
    }

    fn sample() -> Resource {
        let mut m = Mem::new();
        // Root: particle rules, emitter rules, effect rules.
        m.dict(0x100, 0x120, 0x1000); // particle rule at 0x1000
        m.dict(0x140, 0x160, 0x2000); // emitter rule at 0x2000
        m.dict(0x180, 0x1A0, 0x2400); // effect rule at 0x2400
        m.ptr(0x0C, 0x100);
        m.ptr(0x14, 0x140);
        m.ptr(0x18, 0x180);
        // Strings.
        m.bytes(0x3000, b"smoke_puff\0");
        m.bytes(0x3020, b"muz_test\0");
        m.bytes(0x3040, b"pack:/steam_anim.dds\0");
        m.bytes(0x3060, b"DiffuseTex2\0");
        // Particle rule: a sprite with a 7×7 sheet, additive, a growing size.
        m.ptr(0x1000 + 0x124, 0x3000);
        m.bytes(0x1000 + 0x128, b"ptxsprite\0");
        m.u32(0x1000 + 0x104, 7);
        for (k, v) in [3u32, 48, 7, 7].iter().enumerate() {
            m.u32(0x1000 + 0x174 + k as u64 * 4, *v);
        }
        for k in 0..24u64 {
            m.u32(0x1000 + 0x198 + k * 0x34 + 0x28, k as u32);
        }
        m.curve(0x1000 + 0x198 + 4 * 0x34, 0x3400, &[(0.0, [0.1, 0.1, 0.2, 0.2]), (1.0, [0.5, 0.5, 1.0, 1.0])]);
        // Shader with one texture variable.
        m.ptr(0x1678 + 0x14, 0x3100);
        m.u32(0x1678 + 0x18, 0x0001_0001);
        m.ptr(0x3100, 0x3200);
        m.ptr(0x3200 + 0x08, 0x3060);
        m.u32(0x3200 + 0x10, 0x0606_0000);
        m.ptr(0x3200 + 0x24, 0x3040);
        // Emitter rule: one shot of five particles living 0.8 to 1.8 s.
        m.f32(0x2000 + 0x08, 0.01);
        m.ptr(0x2000 + 0x1C, 0x2300);
        m.ptr(0x2000 + 0x1E0, 0x3000);
        m.bytes(0x2000 + 0x1EC, &[1]);
        m.curve(0x2000 + 0x24, 0x3500, &[(0.0, [5.0, 5.0, 0.0, 0.0])]);
        m.curve(0x2000 + 0x24 + 3 * 0x28, 0x3530, &[(0.0, [0.8, 1.8, 0.0, 0.0])]);
        // Target domain: velocity along +X.
        m.u32(0x2300 + 0x08, 2);
        m.curve(0x2300 + 0x10, 0x3600, &[(0.0, [0.7, 0.3, 0.0, 0.0])]);
        // Effect rule with one emitter event and one other event.
        m.ptr(0x2400 + 0xF8, 0x3020);
        m.ptr(0x2400 + 0x130, 0x2600);
        m.u32(0x2400 + 0x134, 0x0002_0002);
        m.ptr(0x2600, 0x2700);
        m.ptr(0x2604, 0x2800);
        m.ptr(0x2700 + 0x48, 0x2000);
        m.ptr(0x2700 + 0x4C, 0x1000);
        m.f32(0x2700 + 0x28, 1.5);
        m.f32(0x2700 + 0x2C, 1.6);
        Resource { version: TYPE, system: m.0, graphics: Vec::new() }
    }

    #[test]
    fn reads_effects_emitters_and_particles() {
        let list = parse(&sample()).unwrap();
        assert!(list.errors.is_empty(), "{:?}", list.errors);
        let fx = list.effect("FX_MUZ_test").expect("found by its cutscene name");
        assert_eq!(fx.events.len(), 1);
        assert_eq!(list.other_events, 1);
        let ev = &fx.events[0];
        assert_eq!(ev.zoom, [1.5, 1.6]);
        let em = &list.emitters[ev.emitter];
        assert!(em.one_shot);
        assert_eq!(em.spawn_rate.at(0.0), [5.0, 5.0, 0.0, 0.0]);
        assert_eq!(em.life.at(0.5)[..2], [0.8, 1.8]);
        assert_eq!(em.target.as_ref().unwrap().position.at(0.0)[0], 0.7);
        assert!(em.creation.is_none());
        let p = &list.particles[ev.particle];
        assert_eq!(p.kind, ParticleKind::Sprite);
        assert!(p.additive);
        assert_eq!(p.texture.as_deref(), Some("steam_anim"));
        assert_eq!(p.frames, Frames { start_max: 3, last: 48, columns: 7, rows: 7 });
        assert_eq!(p.property(prop::SIZE).at(0.5), [0.3, 0.3, 0.6, 0.6]);
        assert!(p.property(prop::ALPHA).0.is_empty());
        assert!((fx.duration(&list) - 1.81).abs() < 1e-5);
    }

    #[test]
    fn curves_interpolate_and_hold() {
        let k = Keyframes(vec![
            Key { time: 0.25, value: [1.0, 0.0, 0.0, 0.0] },
            Key { time: 0.75, value: [3.0, 2.0, 0.0, 0.0] },
        ]);
        assert_eq!(k.at(0.0)[0], 1.0);
        assert_eq!(k.at(0.5)[..2], [2.0, 1.0]);
        assert_eq!(k.at(1.0)[0], 3.0);
        assert_eq!(Keyframes::default().at(0.5), [0.0; 4]);
    }

    #[test]
    fn rejects_other_resource_types() {
        let mut res = sample();
        res.version = 27;
        assert!(parse(&res).is_err());
    }

    #[test]
    fn strips_the_cutscene_prefix() {
        assert_eq!(strip_fx("FX_muz_machine"), "muz_machine");
        assert_eq!(strip_fx("fx_cs_x"), "cs_x");
        assert_eq!(strip_fx("muz"), "muz");
    }
}
