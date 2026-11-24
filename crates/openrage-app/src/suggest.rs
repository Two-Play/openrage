//! Which models an animation dictionary (`.ycd`) can be played on.
//!
//! A clip binds to a skeleton through bone tags. Human bones have fixed tags
//! (`SKEL_Pelvis` = `0x2E28`, …); every other bone's tag is a hash of its name
//! (see [`bone_tag`]), so a vehicle door is `door_dside_f` = `0xEE23` on every
//! vehicle. The animated tags therefore tell what kind of model an animation
//! is made for, and how well a candidate model fits: the share of the
//! animation's bones its skeleton has. Names only pick candidates —
//! `veh@…` dictionaries, for instance, mostly animate peds sitting in
//! vehicles, while cutscene clips are named after the model they move
//! (`coquette4-0`, `a_f_y_carclub_01^1_dual-0`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use rage_crypto::GtaVKeys;
use rage_formats::clip::{Animation, ClipDictionary, TRACK_BONE_ROTATION, TRACK_BONE_SCALE, TRACK_BONE_TRANSLATION};
use rage_formats::drawable;
use rage_rpf::vfs::Node;

/// What an animation moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// The human skeleton (all peds share it).
    Ped,
    /// Vehicle bones (doors, roofs, wheels, turrets, …).
    Vehicle,
    /// Anything else: props and other objects with skeletons.
    Prop,
}

/// The bone tag of a non-human bone: `ElfHash(UPPERCASE(name)) % 0xFE8F + 0x170`.
pub fn bone_tag(name: &str) -> u16 {
    let mut h: u32 = 0;
    for c in name.bytes() {
        h = (h << 4).wrapping_add(u32::from(c.to_ascii_uppercase()));
        let x = h & 0xF000_0000;
        if x != 0 {
            h ^= x >> 24;
        }
        h &= !x;
    }
    (h % 0xFE8F + 0x170) as u16
}

/// Tags of the human skeleton (`SKEL_*`, `IK_*`, `PH_*`, `MH_*`, `RB_*`,
/// facial bones), without the root.
pub const PED_TAGS: [u16; 97] = [
    0x2E28, 0xE39F, 0xF9BB, 0x3779, 0x083C, 0xFEDD, 0xE175, 0xB3FE, 0xCA72, 0x9000, 0xCC4D, 0x512D, 0x8AAE, 0x60E6,
    0x3FCF, 0x5C57, 0x192A, 0xE0FD, 0x5C01, 0x60F0, 0x60F1, 0x60F2, 0xFCD9, 0xB1C5, 0xEEEB, 0x49D9, 0x67F2, 0x0FF9,
    0x0FFA, 0x67F3, 0x1049, 0x104A, 0x67F4, 0x1059, 0x105A, 0x67F5, 0x1029, 0x102A, 0x67F6, 0x1039, 0x103A, 0xEB95,
    0x8CBD, 0xEE4F, 0x1470, 0x58B7, 0x29D2, 0x9D4D, 0x6E5C, 0xDEAD, 0xE5F2, 0xFA10, 0xFA11, 0xE5F3, 0xFA60, 0xFA61,
    0xE5F4, 0xFA70, 0xFA71, 0xE5F5, 0xFA40, 0xFA41, 0xE5F6, 0xFA50, 0xFA51, 0x6F06, 0x188E, 0xAB22, 0x90FF, 0x0BB0,
    0x9995, 0x796E, 0x322C, 0xFE2C, 0xE3DB, 0xB2B6, 0x62AC, 0x542E, 0x74AC, 0xAA10, 0x6B52, 0x4B88, 0x054C, 0x2BA6,
    0x9149, 0x4ED2, 0xF18F, 0x4F37, 0x4537, 0xB4A0, 0x4324, 0x508F, 0xB93B, 0xC33B, 0xB987, 0x8B93, 0xDD1C,
];

/// The main human bones: pelvis, legs, spine, arms, hands, neck and head.
const PED_CORE: [u16; 23] = [
    0x2E28, 0xE39F, 0xCA72, 0xF9BB, 0x9000, 0x3779, 0xCC4D, 0xE0FD, 0x5C01, 0x60F0, 0x60F1, 0x60F2, 0xFCD9, 0x29D2,
    0xB1C5, 0x9D4D, 0xEEEB, 0x6E5C, 0x49D9, 0xDEAD, 0x9995, 0x796E, 0x322C,
];

/// Main bones of Red Dead Redemption 2's people and animals. Their tags are
/// hashes of the names like every other bone's ([`bone_tag`]), not GTA V's
/// fixed ones; animals share these names, so they tell peds from objects
/// but not people from animals.
const RDR2_SKEL_CORE: [&str; 21] = [
    "SKEL_Pelvis",
    "SKEL_Spine_Root",
    "SKEL_Spine0",
    "SKEL_Spine1",
    "SKEL_Spine2",
    "SKEL_Spine3",
    "SKEL_Neck0",
    "SKEL_Neck1",
    "SKEL_Head",
    "SKEL_L_Thigh",
    "SKEL_L_Calf",
    "SKEL_L_Foot",
    "SKEL_R_Thigh",
    "SKEL_R_Calf",
    "SKEL_R_Foot",
    "SKEL_L_Clavicle",
    "SKEL_L_UpperArm",
    "SKEL_L_Forearm",
    "SKEL_R_Clavicle",
    "SKEL_R_UpperArm",
    "SKEL_R_Forearm",
];

fn rdr2_core() -> &'static HashSet<u16> {
    static TAGS: OnceLock<HashSet<u16>> = OnceLock::new();
    TAGS.get_or_init(|| RDR2_SKEL_CORE.iter().map(|n| bone_tag(n)).collect())
}

/// The human skeleton's tags as a set (for ranking peds without loading them).
pub fn ped_skeleton() -> Arc<HashSet<u16>> {
    static TAGS: OnceLock<Arc<HashSet<u16>>> = OnceLock::new();
    TAGS.get_or_init(|| Arc::new(PED_TAGS.iter().copied().collect())).clone()
}

/// Tags of common vehicle bones.
fn vehicle_tags() -> &'static HashSet<u16> {
    static TAGS: OnceLock<HashSet<u16>> = OnceLock::new();
    TAGS.get_or_init(|| {
        let mut names: Vec<String> = [
            "chassis_dummy",
            "chassis_lowlod",
            "bodyshell",
            "bonnet",
            "boot",
            "roof",
            "roof2",
            "windscreen",
            "windscreen_r",
            "engine",
            "petrolcap",
            "steeringwheel",
            "steering",
            "spoiler",
            "struts",
            "bumper_f",
            "bumper_r",
            "wing_lf",
            "wing_rf",
            "exhaust",
            "handlebars",
            "forks_u",
            "forks_l",
            "swingarm",
            "rudder",
            "rudder2",
            "aileron_l",
            "aileron_r",
            "elevator_l",
            "elevator_r",
            "airbrake_l",
            "airbrake_r",
            "flap_l",
            "flap_r",
            "rotor_main",
            "rotor_rear",
            "propeller",
            "gear_f",
            "gear_rl",
            "gear_rr",
            "gear_door_fl",
            "gear_door_fr",
            "gear_door_rl",
            "gear_door_rr",
            "interiorlight",
            "dashglow",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for side in ["dside", "pside"] {
            for end in ["f", "r"] {
                names.push(format!("door_{side}_{end}"));
                names.push(format!("handle_{side}_{end}"));
                names.push(format!("seat_{side}_{end}"));
            }
        }
        for w in ["lf", "rf", "lr", "rr", "lm", "rm", "lm1", "rm1"] {
            names.push(format!("window_{w}"));
            names.push(format!("wheel_{w}"));
            names.push(format!("suspension_{w}"));
            names.push(format!("hub_{w}"));
        }
        for i in 1..=40 {
            names.push(format!("soft_{i}"));
            names.push(format!("extra_{i}"));
        }
        for i in 1..=4 {
            names.push(format!("turret_{i}base"));
            names.push(format!("turret_{i}barrel"));
            names.push(format!("prop_{i}"));
            names.push(format!("rotor_main_{i}"));
        }
        for c in 'a'..='z' {
            names.push(format!("misc_{c}"));
        }
        names.iter().map(|n| bone_tag(n)).collect()
    })
}

/// The bones an animation moves (translation, rotation or scale tracks), without the root.
pub fn animation_tags(animation: &Animation) -> HashSet<u16> {
    animation
        .tracks
        .iter()
        .filter(|t| t.bone != 0 && matches!(t.track, TRACK_BONE_TRANSLATION | TRACK_BONE_ROTATION | TRACK_BONE_SCALE))
        .map(|t| t.bone)
        .collect()
}

/// What the bones `tags` belong to (`None` without any bone: mover or facial tracks only).
pub fn classify(tags: &HashSet<u16>) -> Option<Kind> {
    if tags.is_empty() {
        return None;
    }
    let ped = tags.iter().filter(|t| PED_TAGS.contains(t)).count();
    let core = tags.iter().filter(|t| PED_CORE.contains(t) || rdr2_core().contains(t)).count();
    let vehicle = tags.iter().filter(|t| vehicle_tags().contains(t)).count();
    // Cutscene peds also move a full facial rig of their own, so the main
    // body bones count on their own.
    Some(if ped * 2 >= tags.len() || core >= 3 {
        Kind::Ped
    } else if vehicle > ped && vehicle * 4 >= tags.len() {
        Kind::Vehicle
    } else {
        Kind::Prop
    })
}

fn stem(ycd: &str) -> String {
    let name = ycd.rsplit('/').next().unwrap_or(ycd).to_ascii_lowercase();
    name.strip_suffix(".ycd").unwrap_or(&name).to_owned()
}

/// Words of dictionary names that never name a model.
const GENERIC: [&str; 40] = [
    "anim",
    "veh",
    "vehicle",
    "vehicles",
    "amb",
    "world",
    "human",
    "male",
    "female",
    "base",
    "idle",
    "idles",
    "enter",
    "exit",
    "enter_exit",
    "common",
    "front",
    "rear",
    "left",
    "right",
    "side",
    "low",
    "std",
    "fps",
    "mini",
    "missheist",
    "mp",
    "player",
    "intveh",
    "scripted",
    "move",
    "facials",
    "generic",
    "props",
    "prop",
    "intro",
    "outro",
    "loop",
    "start",
    "end",
];

/// The model a cutscene clip is named after: `a_f_y_carclub_01^1_dual-0` →
/// `a_f_y_carclub_01`, `coquette4-0` → `coquette4`. Other clips name none.
pub fn clip_model(clip: &str) -> Option<String> {
    let (name, section) = clip.rsplit_once('-')?;
    if section.is_empty() || !section.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let name = name.split('^').next().unwrap_or(name);
    let name = name.strip_suffix("_dual").unwrap_or(name);
    (name.len() > 2).then(|| name.to_ascii_lowercase())
}

/// A clip named after the object it moves (RDR2's doors and destruction:
/// `p_door_barnslide01x`, `des_brt3_door_01`).
fn prop_clip(clip: &str) -> Option<String> {
    let name = clip.to_ascii_lowercase();
    let prop = ["p_", "prop_", "des_", "s_"].iter().any(|p| name.starts_with(p));
    (prop && !name.contains(['@', '/', ' '])).then_some(name)
}

/// Model names a dictionary name hints at, most specific first:
/// `va_zion2` → `zion2`; `vehicle_cutter` → `cutter`;
/// `anim@veh@armordillo@turret@base` → `armordillo`, `turret`.
pub fn name_hints(ycd: &str) -> Vec<String> {
    let name = stem(ycd);
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim_matches(|c: char| c == '_' || c == '.');
        if s.len() > 2 && !GENERIC.contains(&s) && !s.chars().all(|c| c.is_ascii_digit()) && !out.iter().any(|o| o == s)
        {
            out.push(s.to_owned());
        }
    };
    if let Some(v) = name.strip_prefix("va_") {
        push(v);
    }
    push(&name);
    for segment in name.split('@') {
        push(segment);
        for word in segment.split('_') {
            push(word);
        }
    }
    out
}

/// What a dictionary animates, and the model names it hints at.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Bone tags and kind of each animation that moves bones.
    pub animations: Vec<(HashSet<u16>, Kind)>,
    /// Kinds of models the animations are for, most animations first.
    pub kinds: Vec<Kind>,
    /// An animal (`creatures@<animal>@…`), suggested as `a_c_<animal>`.
    pub animal: Option<String>,
    /// Clip names hint at female peds.
    pub female: bool,
    /// Model names from cutscene clip names, then from the dictionary name.
    pub hints: Vec<String>,
    /// Moves RDR2's people or animals (their main bones, see [`RDR2_SKEL_CORE`]).
    pub rdr2_peds: bool,
    /// For RDR2's people and animals: every bone the dictionary moves.
    all_bones: HashSet<u16>,
}

impl Profile {
    pub fn new(ycd: &str, dict: &ClipDictionary) -> Self {
        let clips: Vec<&str> = dict.clips.iter().map(|c| c.name.as_str()).collect();
        Self::from_tags(ycd, dict.animations.iter().map(animation_tags).collect(), &clips)
    }

    /// From the bone tags of each animation and the clip names.
    pub fn from_tags(ycd: &str, animations: Vec<HashSet<u16>>, clips: &[&str]) -> Self {
        let name = stem(ycd);
        let animations: Vec<(HashSet<u16>, Kind)> =
            animations.into_iter().filter_map(|t| classify(&t).map(|k| (t, k))).collect();
        let mut counts: HashMap<Kind, usize> = HashMap::new();
        for (_, k) in &animations {
            *counts.entry(*k).or_default() += 1;
        }
        let mut kinds: Vec<(Kind, usize)> = counts.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut kinds: Vec<Kind> = kinds.into_iter().map(|k| k.0).collect();
        if name.starts_with("va_") && !kinds.contains(&Kind::Vehicle) {
            kinds.insert(0, Kind::Vehicle);
        }
        // Only mover or facial tracks: those are for peds.
        if kinds.is_empty() {
            kinds.push(Kind::Ped);
        }
        let animal = name.split("creatures@").nth(1).map(|rest| rest.split('@').next().unwrap_or(rest).to_owned());
        let female = name.contains("_f@") || name.contains("female") || name.starts_with("move_f");
        let mut hints: Vec<String> = Vec::new();
        for h in clips.iter().filter_map(|c| clip_model(c).or_else(|| prop_clip(c))).chain(name_hints(ycd)) {
            if !hints.contains(&h) {
                hints.push(h);
            }
        }
        let rdr2_peds = animations.iter().any(|(t, _)| t.iter().filter(|t| rdr2_core().contains(t)).count() >= 3);
        let all_bones =
            if rdr2_peds { animations.iter().flat_map(|(t, _)| t.iter().copied()).collect() } else { HashSet::new() };
        Profile { animations, kinds, animal, female, hints, rdr2_peds, all_bones }
    }

    pub fn describe(&self) -> String {
        if let Some(a) = &self.animal {
            return format!("Animal animation ({a})");
        }
        // Facial tracks of RDR2's people count as objects: say what the body is.
        if self.rdr2_peds {
            return "Animation of people or animals".into();
        }
        let names: Vec<&str> = self
            .kinds
            .iter()
            .map(|k| match k {
                Kind::Ped => "peds",
                Kind::Vehicle => "vehicles",
                Kind::Prop => "objects",
            })
            .collect();
        match self.kinds.as_slice() {
            [Kind::Ped] => "Human animation — fits all peds".into(),
            [Kind::Vehicle] => "Vehicle animation".into(),
            [Kind::Prop] => "Object animation".into(),
            _ => format!("Animates {}", names.join(", ")),
        }
    }

    /// How well a skeleton fits: per mille of the best-fitting animation's
    /// bones it has, the number of those bones, and what that animation is for.
    ///
    /// RDR2's people and animals share the main bones' names, so a cow has
    /// every bone of a clip that moves only the pelvis and spine. For them
    /// the share of all the bones the dictionary moves counts instead (a
    /// person has the fingers and face bones too).
    pub fn fit(&self, skeleton: &HashSet<u16>) -> (usize, usize, Option<Kind>) {
        if self.rdr2_peds {
            let hits = self.all_bones.iter().filter(|t| skeleton.contains(t)).count();
            return (hits * 1000 / self.all_bones.len().max(1), hits, Some(Kind::Ped));
        }
        self.animations
            .iter()
            .map(|(a, k)| {
                let hits = a.iter().filter(|t| skeleton.contains(t)).count();
                (hits * 1000 / a.len().max(1), hits, Some(*k))
            })
            .max()
            .unwrap_or((0, 0, None))
    }
}

/// A model that might fit, with its skeleton's bone tags if known.
pub struct Candidate {
    pub vpath: String,
    pub tags: Option<Arc<HashSet<u16>>>,
}

/// File name without extension, lower case.
pub fn model_stem(vpath: &str) -> String {
    let name = vpath.rsplit('/').next().unwrap_or(vpath).to_ascii_lowercase();
    name.split('.').next().unwrap_or(&name).to_owned()
}

/// Orders candidates best first. Models named by a hint (a cutscene clip,
/// `va_<vehicle>`) come first, in hint order, if their skeleton has at least
/// a quarter of an animation's bones. The rest follow grouped by what they
/// fit (the dictionary's main kind first), then by the share of bones they
/// have, otherwise in the given order. Models with none of the bones are
/// dropped when others have some; of several files with the same name the
/// best fitting one stays (the base game's if equal).
pub fn rank(profile: &Profile, candidates: Vec<Candidate>) -> Vec<String> {
    struct Scored {
        fit: (usize, usize, Option<Kind>),
        hint: Option<usize>,
        update: bool,
        vpath: String,
    }
    let scored: Vec<Scored> = candidates
        .into_iter()
        .map(|c| {
            let fit = c.tags.as_ref().map_or((0, 0, None), |s| profile.fit(s));
            let stem = model_stem(&c.vpath);
            let hint = profile.hints.iter().position(|h| *h == stem);
            Scored { fit, hint, update: c.vpath.starts_with("update"), vpath: c.vpath }
        })
        .collect();
    // One file per model name: the best fitting, the base game's before an update's.
    let mut first: HashMap<String, usize> = HashMap::new();
    let mut unique: Vec<Scored> = Vec::new();
    for s in scored {
        match first.get(&model_stem(&s.vpath)) {
            Some(&i) => {
                let u = &unique[i];
                if (s.fit.0, !s.update, std::cmp::Reverse(s.vpath.len()))
                    > (u.fit.0, !u.update, std::cmp::Reverse(u.vpath.len()))
                {
                    unique[i] = s;
                }
            }
            None => {
                first.insert(model_stem(&s.vpath), unique.len());
                unique.push(s);
            }
        }
    }
    let mut scored = unique;
    let any_fit = scored.iter().any(|s| s.fit.0 > 0);
    if any_fit {
        scored.retain(|s| s.fit.0 > 0);
    }
    let key = |s: &Scored| {
        let named = s.hint.filter(|_| s.fit.0 >= 250 || !any_fit);
        let kind = s.fit.2.and_then(|k| profile.kinds.iter().position(|p| *p == k)).unwrap_or(usize::MAX);
        // Close fits count as equal.
        (
            named.is_none(),
            named.unwrap_or(0),
            kind,
            std::cmp::Reverse(s.fit.0 / 50),
            s.hint.unwrap_or(usize::MAX),
            std::cmp::Reverse(s.fit.1),
        )
    };
    // Stable: otherwise the order of the candidates stays (e.g. female peds first).
    scored.sort_by_cached_key(key);
    scored.into_iter().map(|s| s.vpath).collect()
}

/// Each clip's name and the bones of its (first) animation.
pub fn clip_tags(dict: &ClipDictionary) -> Vec<(String, HashSet<u16>)> {
    dict.clips
        .iter()
        .map(|c| {
            let tags = c.parts.first().and_then(|p| dict.animations.get(p.animation)).map(animation_tags);
            (c.name.clone(), tags.unwrap_or_default())
        })
        .collect()
}

/// The clip (of [`clip_tags`]) that fits a model best: most of its bones
/// animated, then a clip named after the model, then the first.
pub fn best_clip(clips: &[(String, HashSet<u16>)], skeleton: &HashSet<u16>, model: &str) -> usize {
    let model = model_stem(model);
    clips
        .iter()
        .enumerate()
        .max_by_key(|(i, (name, tags))| {
            let hits = tags.iter().filter(|t| skeleton.contains(t)).count();
            // A cutscene section (`csb_bogdan_dual-3`) or a whole joined
            // cutscene clip (`csb_bogdan_dual`, `prop^1`) named after the model.
            let joined = || {
                let n = name.split('^').next().unwrap_or(name);
                n.strip_suffix("_dual").unwrap_or(n).to_ascii_lowercase()
            };
            let named = clip_model(name).unwrap_or_else(joined) == model;
            // The model's own clip wins as long as it moves some of its bones.
            (named && hits > 0, hits, std::cmp::Reverse(*i))
        })
        .map_or(0, |(i, _)| i)
}

/// The bone tags of a model's skeleton (cached). Pedestrian drawable
/// dictionaries take the skeleton from the fragment next to them.
pub fn skeleton_tags(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Option<Arc<HashSet<u16>>> {
    type Cache = Mutex<HashMap<String, Option<Arc<HashSet<u16>>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok()?.get(vpath) {
        return hit.clone();
    }
    let load = || -> Option<drawable::Skeleton> {
        let (node, _) = rage_mods::resolve(game_dir, vpath, keys).ok()?;
        if let Node::Rpf8File(rpf, i) = node {
            return rdr2_skeleton(&rpf, i);
        }
        let Node::ArchiveFile(rpf, id) = node else { return None };
        let res = rpf.read_resource(id).ok()?;
        let lower = vpath.to_ascii_lowercase();
        if lower.ends_with(".yft") {
            return drawable::parse_fragment(&res).ok()?.drawable.skeleton;
        }
        if lower.ends_with(".ydr") {
            return drawable::parse_ydr(&res).ok()?.skeleton;
        }
        let own = drawable::parse_ydd(&res).ok()?.into_iter().find_map(|(_, d)| d.skeleton);
        own.or_else(|| {
            let dir = rpf.entry(id).parent.unwrap_or(rage_rpf::Rpf7::ROOT);
            let tid = rpf.find_child(dir, &format!("{}.yft", model_stem(vpath)))?;
            drawable::parse_fragment(&rpf.read_resource(tid).ok()?).ok()?.drawable.skeleton
        })
    };
    let tags = load().map(|s| Arc::new(s.bones.iter().map(|b| b.tag).filter(|&t| t != 0).collect()));
    if let Ok(mut c) = cache.lock() {
        c.insert(vpath.to_owned(), tags.clone());
    }
    tags
}

/// The skeleton of an RDR2 model: a fragment's, a drawable's, a drawable
/// dictionary's own or its ped's base skeleton, or a ped's (outfit file).
fn rdr2_skeleton(rpf: &rage_rpf::Rpf8, i: usize) -> Option<drawable::Skeleton> {
    let full = rpf.entry_name(i);
    let stem = model_stem(&full);
    if crate::models::is_rdr2_ped_file(&full, rpf.display_name()) {
        return crate::models::rdr2_ped_skeleton(&crate::rdr2::game_folder(rpf.path())?, &stem);
    }
    let res = rpf.read_resource(rpf.entries()?.get(i)?).ok()?;
    let lower = full.to_ascii_lowercase();
    if lower.ends_with(".yft") {
        return drawable::parse_fragment_rdr2(&res).ok()?.drawable.skeleton;
    }
    if lower.ends_with(".ydr") {
        return drawable::parse_ydr_rdr2(&res).ok()?.skeleton;
    }
    let own = drawable::parse_ydd_rdr2(&res).ok()?.into_iter().find_map(|(_, d)| d.skeleton);
    own.or_else(|| crate::models::rdr2_base_skeleton(rpf, &stem))
}

/// RDR2's peds and animals as candidates: their outfit files
/// (`…/metapeds.rpf/<ped>.ymt`), each with the tags of its base skeleton.
/// The base skeletons (a few hundred, shared by many peds) are read once,
/// in parallel. The player (Arthur) comes first, then story characters,
/// then the rest.
pub fn rdr2_ped_candidates(game_dir: &Path, outfit_files: Vec<String>) -> Vec<Candidate> {
    use rayon::prelude::*;
    type Cache = Mutex<HashMap<String, Option<Arc<HashSet<u16>>>>>;
    static BASES: OnceLock<Cache> = OnceLock::new();
    let cache = BASES.get_or_init(Default::default);
    let mut files = outfit_files;
    files.sort_by_key(|v| {
        let n = model_stem(v);
        (n != "player_zero", !n.starts_with("cs_"), n)
    });
    files.dedup_by(|a, b| model_stem(a) == model_stem(b));
    let bases: Vec<(String, String)> = files
        .iter()
        .map(|v| {
            let ped = model_stem(v);
            (crate::models::rdr2_ped_base(game_dir, &ped).unwrap_or_default(), ped)
        })
        .collect();
    let tags_of = |s: drawable::Skeleton| Arc::new(s.bones.iter().map(|b| b.tag).filter(|&t| t != 0).collect());
    // Base skeletons not read yet, from the archive of base skeletons.
    let mut missing: Vec<&str> = bases.iter().map(|(b, _)| b.as_str()).filter(|b| !b.is_empty()).collect();
    missing.sort_unstable();
    missing.dedup();
    missing.retain(|b| cache.lock().is_ok_and(|c| !c.contains_key(*b)));
    if let Some(frags) = crate::models::rdr2_base_skeletons(game_dir) {
        let read: Vec<(String, Option<Arc<HashSet<u16>>>)> = missing
            .par_iter()
            .map(|base| {
                let skeleton = frags
                    .find(base)
                    .and_then(|i| frags.entries()?.get(i).cloned())
                    .and_then(|e| frags.read_resource(&e).ok())
                    .and_then(|res| drawable::fragment_skeleton_rdr2(&res).ok().flatten());
                (base.to_string(), skeleton.map(tags_of))
            })
            .collect();
        if let Ok(mut c) = cache.lock() {
            c.extend(read);
        }
    }
    files
        .into_iter()
        .zip(bases)
        .map(|(vpath, (base, ped))| {
            let cached = (!base.is_empty()).then(|| cache.lock().ok().and_then(|c| c.get(&base).cloned())).flatten();
            // Peds missing from the ped list: their skeleton guessed from the name.
            let tags = cached.flatten().or_else(|| crate::models::rdr2_ped_skeleton(game_dir, &ped).map(tags_of));
            Candidate { vpath, tags }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(tags: &[u16]) -> HashSet<u16> {
        tags.iter().copied().collect()
    }

    fn names(list: &[&str]) -> HashSet<u16> {
        list.iter().map(|n| bone_tag(n)).collect()
    }

    /// A profile from `(clip name, animated bones)` pairs.
    fn profile(ycd: &str, clips: &[(&str, &HashSet<u16>)]) -> Profile {
        let names: Vec<&str> = clips.iter().map(|c| c.0).collect();
        Profile::from_tags(ycd, clips.iter().map(|c| c.1.clone()).collect(), &names)
    }

    /// A convertible roof (as in va_zion2): soft-top and window bones.
    fn roof() -> HashSet<u16> {
        names(&["soft_1", "soft_2", "soft_3", "window_lf", "window_rf", "roof"])
    }

    /// A ped sitting in a van (veh@van@…): pelvis, legs, fingers.
    fn sitting() -> HashSet<u16> {
        set(&[0x2E28, 0xE39F, 0xF9BB, 0x9000, 0x0FF9, 0x67F2, 0xDEAD])
    }

    #[test]
    fn bone_tags_match_the_game() {
        // Tags as found in zion2.yft.
        assert_eq!(bone_tag("door_dside_f"), 0xEE23);
        assert_eq!(bone_tag("soft_1"), 0xB8EC);
        assert_eq!(bone_tag("wheel_lf"), 0x6D12);
        assert_eq!(bone_tag("window_lf"), 0x686E);
    }

    #[test]
    fn classifies_by_bones_not_names() {
        assert_eq!(classify(&roof()), Some(Kind::Vehicle));
        assert_eq!(classify(&sitting()), Some(Kind::Ped));
        // Fingers only (pressing the horn) are still human.
        assert_eq!(classify(&set(&[0x0FF9, 0x0FFA, 0x67F2])), Some(Kind::Ped));
        assert_eq!(classify(&names(&["lever", "handle_base"])), Some(Kind::Prop));
        assert_eq!(classify(&HashSet::new()), None);
        // A cutscene ped with a facial rig of many more (hashed) bones.
        let mut face = sitting();
        face.extend(names(&["fb_a", "fb_b", "fb_c", "fb_d", "fb_e", "fb_f", "fb_g", "fb_h", "fb_i", "fb_j"]));
        assert_eq!(classify(&face), Some(Kind::Ped));

        let veh = profile("veh@van@side_fps.ycd", &[("get_in", &sitting())]);
        assert_eq!(veh.kinds, [Kind::Ped]);
        let va = profile("va_zion2.ycd", &[("roof", &roof())]);
        assert_eq!(va.kinds, [Kind::Vehicle]);
        assert_eq!(va.hints[0], "zion2");
        let facial = profile("facials@gen_male@base", &[]);
        assert_eq!(facial.kinds, [Kind::Ped]);
    }

    #[test]
    fn hints_name_models() {
        assert_eq!(name_hints("x64e.rpf/levels/gta5/vehicles.rpf/va_zion2.ycd")[0], "zion2");
        assert!(name_hints("vehicle_cutter").contains(&"cutter".to_owned()));
        let h = name_hints("anim@veh@armordillo@turret@base");
        assert!(h.contains(&"armordillo".to_owned()));
        assert!(!h.iter().any(|w| w == "veh" || w == "base" || w == "anim"));
        assert_eq!(clip_model("a_f_y_carclub_01^1_dual-0").as_deref(), Some("a_f_y_carclub_01"));
        assert_eq!(clip_model("coquette4-0").as_deref(), Some("coquette4"));
        assert_eq!(clip_model("get_in"), None);
    }

    #[test]
    fn ranks_by_shared_bones() {
        let skeleton = |list: &[&str]| Some(Arc::new(names(list)));
        let convertible = ["soft_1", "soft_2", "soft_3", "window_lf", "window_rf", "roof", "door_dside_f"];
        let candidates = || {
            vec![
                Candidate { vpath: "x64e.rpf/peds/a_m_y_hipster_01.ydd".into(), tags: Some(ped_skeleton()) },
                Candidate { vpath: "x64e.rpf/vehicles.rpf/adder.yft".into(), tags: skeleton(&["window_lf", "roof"]) },
                Candidate { vpath: "x64e.rpf/vehicles.rpf/cogcabrio.yft".into(), tags: skeleton(&convertible) },
                Candidate { vpath: "update/x64/vehicles.rpf/zion2.yft".into(), tags: skeleton(&convertible) },
                Candidate { vpath: "x64e.rpf/vehicles.rpf/zion2.yft".into(), tags: skeleton(&convertible) },
            ]
        };
        // A vehicle dictionary: the named vehicle, other convertibles, then
        // vehicles with some of the bones; never a ped.
        let va = profile("va_zion2.ycd", &[("roof", &roof())]);
        assert_eq!(
            rank(&va, candidates()),
            [
                "x64e.rpf/vehicles.rpf/zion2.yft",
                "x64e.rpf/vehicles.rpf/cogcabrio.yft",
                "x64e.rpf/vehicles.rpf/adder.yft"
            ]
        );
        // A ped dictionary inside a vehicle: only the ped.
        let sit = profile("veh@van@side_fps.ycd", &[("sit", &sitting())]);
        assert_eq!(rank(&sit, candidates()), ["x64e.rpf/peds/a_m_y_hipster_01.ydd"]);
        // A cutscene: the models its clips are named after come first.
        let cut = profile("tun_meet_int-0.ycd", &[("a_m_y_hipster_01_dual-0", &sitting()), ("zion2-0", &roof())]);
        assert_eq!(cut.kinds.len(), 2);
        assert_eq!(
            &rank(&cut, candidates())[..2],
            ["x64e.rpf/peds/a_m_y_hipster_01.ydd", "x64e.rpf/vehicles.rpf/zion2.yft"]
        );
    }

    #[test]
    fn prop_clips_name_their_model() {
        let door = set(&[bone_tag("door")]);
        let p = profile("p_door", &[("p_door_barnslide01x", &door), ("idle", &door)]);
        assert_eq!(p.hints[0], "p_door_barnslide01x");
        assert!(!p.hints.contains(&"idle".to_owned()));
    }

    #[test]
    fn rdr2_people_before_animals() {
        let core = ["SKEL_Pelvis", "SKEL_Spine0", "SKEL_Spine1", "SKEL_Spine2", "SKEL_Neck0", "SKEL_Head"];
        let hands = ["SKEL_L_Finger00", "SKEL_L_Finger01", "SKEL_R_Finger00", "SKEL_R_Finger01", "SKEL_Spine6"];
        let body = names(&core);
        let person: HashSet<u16> = names(&core).union(&names(&hands)).copied().collect();
        // A sitting clip: the body alone, then the body with the hands.
        let p = profile("de817d5c", &[("base", &body), ("enter", &person)]);
        assert!(p.rdr2_peds);
        assert_eq!(p.describe(), "Animation of people or animals");
        // A cow has the body bones only; the person every bone.
        let cow = Candidate { vpath: "metapeds.rpf/a_c_cow.ymt".into(), tags: Some(Arc::new(body.clone())) };
        let man = Candidate { vpath: "metapeds.rpf/cs_abe.ymt".into(), tags: Some(Arc::new(person.clone())) };
        assert_eq!(rank(&p, vec![cow, man]), ["metapeds.rpf/cs_abe.ymt", "metapeds.rpf/a_c_cow.ymt"]);
        // GTA V's peds are not RDR2's.
        assert!(!profile("move_m@generic", &[("walk", &set(&PED_CORE))]).rdr2_peds);
    }

    #[test]
    fn picks_the_clip_of_the_model() {
        let d: Vec<(String, HashSet<u16>)> =
            [("a_m_y_hipster_01_dual-0", sitting()), ("zion2-0", roof()), ("cogcabrio-0", roof())]
                .into_iter()
                .map(|(n, t)| (n.to_owned(), t))
                .collect();
        assert_eq!(best_clip(&d, &roof(), "x64e.rpf/vehicles.rpf/zion2.yft"), 1);
        assert_eq!(best_clip(&d, &roof(), "cogcabrio.yft"), 2);
        assert_eq!(best_clip(&d, &ped_skeleton(), "a_m_y_hipster_01.ydd"), 0);
    }
}
