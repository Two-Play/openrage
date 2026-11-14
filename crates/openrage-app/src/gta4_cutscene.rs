//! GTA IV cutscenes (`.cut`): plain text in blocks, and the animation
//! dictionaries (`.wad`) next to them in the same archive (`cuts.img`).
//!
//! A file starts with blocks for the whole cutscene — `[CUTSCENE_HEADER]`,
//! `[MISSION_TEXT_NAME]` (the text table of the subtitles), `[TEXT]`
//! (subtitles: start and length in milliseconds, then the text's key),
//! `[FLAGS]`, `[PLAYER_START]` … — then one or more sections between
//! `[SECTION_START]` and `[SECTION_END]`. A section plays one animation
//! dictionary, `[ANIM]` (`B_1_0` → `b_1_0.wad`), for `[DURATION]`
//! milliseconds; its `[MODELS]` lines name each actor's model, its clip and
//! for people the facial clip (`0 CS_PhilB CS_PhilB_0 cs_philb_head_0`), and
//! `[CAMERA]` the camera's clip. Positions are relative to the section's
//! `[OFFSET]`, turned by `[ORIENT]` degrees. Every block ends with `[/NAME]`;
//! the file is padded after the last section (NUL bytes or left-over memory).
//!
//! An actor is the same in every section it appears in when its clip has the
//! same name up to the section number (`CS_PhilB_0`, `CS_PhilB_1`); the
//! numbers before the model names are only the order within a section.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rage_formats::clip::{Animation, Clip, ClipDictionary, ClipPart};
use rage_rpf::vfs::Node;

use crate::cutscene::{self, Kind};
use crate::gta4::ModelSource;

/// A subtitle: when it shows (seconds into the cutscene), for how long and
/// the key of its text in the text table.
#[derive(Debug, Clone, PartialEq)]
pub struct Subtitle {
    pub start: f32,
    pub duration: f32,
    pub key: String,
}

/// One line of a section's `[MODELS]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutModel {
    /// Its number in the section (what `[VARIATION]` lines refer to).
    pub index: u32,
    /// The model (`CS_PhilB`, `player`, `csOldPhone`).
    pub model: String,
    /// Its clip in the section's animation (`CS_PhilB_0`).
    pub anim: String,
    /// People: the facial clip (`cs_philb_head_0`).
    pub face: Option<String>,
}

impl CutModel {
    /// Which actor this is: the clip's name without the section number,
    /// lower case (`CS_PhilB_0` → `cs_philb`).
    pub fn actor(&self) -> String {
        actor_key(&self.anim)
    }
}

/// The clip name without its section number, lower case.
fn actor_key(anim: &str) -> String {
    let lower = anim.to_ascii_lowercase();
    match lower.rsplit_once('_') {
        Some((stem, n)) if !stem.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            stem.to_owned()
        }
        _ => lower,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    pub models: Vec<CutModel>,
    /// Where the section's origin is in the world.
    pub offset: [f32; 3],
    /// The section's rotation about Z, degrees.
    pub orient: f32,
    /// Length in seconds.
    pub duration: f32,
    /// The animation dictionary, without `.wad` (`B_1_0`).
    pub anim: String,
    /// The camera's clip (`camera_0`).
    pub camera: Option<String>,
    /// The audio track (`B_1`).
    pub audio: Option<String>,
    /// Map objects the cutscene hides (its own copies replace them): where
    /// they are and their model.
    pub remove: Vec<([f32; 3], String)>,
    /// Outfits of the section's people (`[VARIATION]`, before the section
    /// or in it).
    pub variations: Vec<Variation>,
}

/// One `[VARIATION]` line: `model component drawable texture [time]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Variation {
    /// The model's number in the section's `[MODELS]`.
    pub model: u32,
    /// The ped component: 0 head, 1 upper body, 2 legs, 3 `suse`, 4 hands,
    /// 5 feet, 6 jacket, 7 hair, 8 `sus2`, 9 teeth, 10 face.
    pub component: u32,
    pub drawable: u32,
    /// Texture variant: 0 `a`, 1 `b`, …
    pub texture: u32,
    /// When it applies, seconds into the section (mostly 0).
    pub time: f32,
}

/// The names of GTA IV's ped components, by number.
const COMPONENTS: [&str; 11] = ["head", "uppr", "lowr", "suse", "hand", "feet", "jckt", "hair", "sus2", "teef", "face"];

impl Variation {
    /// The component's name in the ped's drawables (`hand`).
    pub fn component_name(&self) -> Option<&'static str> {
        COMPONENTS.get(self.component as usize).copied()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gta4Cutscene {
    /// The text table that holds the subtitles (`B1AUD`).
    pub text_table: Option<String>,
    pub subtitles: Vec<Subtitle>,
    pub flags: Vec<String>,
    pub player_start: Option<[f32; 3]>,
    pub sections: Vec<Section>,
}

impl Gta4Cutscene {
    /// Length in seconds: all sections in a row.
    pub fn duration(&self) -> f32 {
        self.sections.iter().map(|s| s.duration).sum()
    }

    /// What an actor (see [`CutModel::actor`]) wears from the start: the
    /// outfit of the first section it is in, (component, drawable, texture).
    /// Changes later on are left out.
    pub fn outfit(&self, actor: &str) -> Vec<(&'static str, u32, u32)> {
        let Some((section, model)) =
            self.sections.iter().find_map(|s| Some((s, s.models.iter().find(|m| m.actor() == actor)?)))
        else {
            return Vec::new();
        };
        section
            .variations
            .iter()
            .filter(|v| v.model == model.index && v.time <= 0.0)
            .filter_map(|v| Some((v.component_name()?, v.drawable, v.texture)))
            .collect()
    }

    /// Every actor (see [`CutModel::actor`]) in the order they first appear,
    /// with its model.
    pub fn actors(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for m in self.sections.iter().flat_map(|s| &s.models) {
            let key = m.actor();
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, m.model.clone()));
            }
        }
        out
    }
}

fn floats<const N: usize>(line: &str) -> Option<[f32; N]> {
    let mut it = line.split(|c: char| c.is_whitespace() || c == ',').filter(|v| !v.is_empty()).map(|v| v.parse().ok());
    let out: [Option<f32>; N] = std::array::from_fn(|_| it.next().flatten());
    if out.iter().any(Option::is_none) {
        return None;
    }
    Some(out.map(|v| v.unwrap_or_default()))
}

/// Reads a `.cut` file. The text ends at the first NUL byte or at text
/// outside a block after the last section (padding).
pub fn parse(data: &[u8]) -> Result<Gta4Cutscene> {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let text: String = data[..end].iter().map(|&b| b as char).collect();
    let mut cut = Gta4Cutscene::default();
    let mut section: Option<Section> = None;
    let mut block: Option<String> = None;
    let mut variations: Vec<Variation> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let at = || format!("line {}", n + 1);
        if line.is_empty() {
            continue;
        }
        match line {
            "[SECTION_START]" => {
                if section.is_some() {
                    bail!("{}: a section starts inside another one", at());
                }
                // Outfits given before the section are its own.
                section = Some(Section { variations: std::mem::take(&mut variations), ..Section::default() });
                block = None;
                continue;
            }
            "[SECTION_END]" => {
                let s = section.take().with_context(|| format!("{}: a section ends that never started", at()))?;
                cut.sections.push(s);
                block = None;
                continue;
            }
            _ => {}
        }
        // A closing tag ends the open block, even with another name (a few
        // files close `[TEXT]` with `[/MISSION_TEXT_NAME]`).
        if line.starts_with("[/") && line.ends_with(']') {
            block = None;
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            block = Some(name.to_owned());
            continue;
        }
        let Some(name) = block.as_deref() else {
            // Padding after the last section (not always NUL bytes).
            if section.is_none() && !cut.sections.is_empty() {
                break;
            }
            // Notes left by the game's tools (`VARIATION FOUND!`).
            continue;
        };
        let bad = || format!("{}: unexpected [{name}] line: {line}", at());
        match (section.as_mut(), name) {
            (None, "MISSION_TEXT_NAME") => cut.text_table = Some(line.to_owned()),
            (None, "TEXT") => {
                let f: Vec<&str> = line.split_whitespace().collect();
                let [start, length, key] = f.as_slice() else { bail!(bad()) };
                let (start, length): (f32, f32) = (start.parse().with_context(bad)?, length.parse().with_context(bad)?);
                cut.subtitles.push(Subtitle {
                    start: start / 1000.0,
                    duration: length / 1000.0,
                    key: (*key).to_owned(),
                });
            }
            (None, "FLAGS") => cut.flags.push(line.to_owned()),
            (None, "PLAYER_START") => cut.player_start = Some(floats(line).with_context(bad)?),
            (Some(s), "MODELS") => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 3 || f[0].parse::<u32>().is_err() {
                    bail!(bad());
                }
                // A fourth field is the facial clip, unless it is a number (a flag) or `null`.
                let face = f
                    .get(3)
                    .filter(|v| v.parse::<f32>().is_err() && !v.eq_ignore_ascii_case("null"))
                    .map(|v| (*v).to_owned());
                let index = f[0].parse().with_context(bad)?;
                s.models.push(CutModel { index, model: f[1].to_owned(), anim: f[2].to_owned(), face });
            }
            (Some(s), "OFFSET") => s.offset = floats(line).with_context(bad)?,
            (Some(s), "ORIENT") => s.orient = floats::<1>(line).with_context(bad)?[0],
            (Some(s), "DURATION") => s.duration = floats::<1>(line).with_context(bad)?[0] / 1000.0,
            (Some(s), "ANIM") => s.anim = line.to_owned(),
            (Some(s), "CAMERA") => s.camera = Some(line.to_owned()),
            (Some(s), "AUDIO") => s.audio = Some(line.to_owned()),
            (s, "VARIATION") => {
                let f: Vec<f32> =
                    line.split_whitespace().map(|v| v.parse()).collect::<Result<_, _>>().with_context(bad)?;
                let (&[model, component, drawable, texture], time) = (&f[..f.len().min(4)], f.get(4)) else {
                    bail!(bad())
                };
                let v = Variation {
                    model: model as u32,
                    component: component as u32,
                    drawable: drawable as u32,
                    texture: texture as u32,
                    time: time.copied().unwrap_or(0.0) / 1000.0,
                };
                match s {
                    Some(s) => s.variations.push(v),
                    None => variations.push(v),
                }
            }
            (Some(s), "REMOVE") => {
                let (at, model) = line.rsplit_once(char::is_whitespace).with_context(bad)?;
                s.remove.push((floats(at).with_context(bad)?, model.to_owned()));
            }
            // Draw distances, lights, effects, vehicles, objects to remove, …
            _ => {}
        }
    }
    if section.is_some() {
        bail!("the last section has no [SECTION_END]");
    }
    if cut.sections.is_empty() {
        bail!("no sections ([SECTION_START] … [SECTION_END])");
    }
    Ok(cut)
}

/// The folder of the episode a virtual path is in (`TLAD/`, `TBoGT/`), or
/// `""` for the base game.
pub fn episode(vpath: &str) -> &str {
    match vpath.split_once('/') {
        Some((first, _)) if first.eq_ignore_ascii_case("TLAD") || first.eq_ignore_ascii_case("TBoGT") => {
            &vpath[..first.len() + 1]
        }
        _ => "",
    }
}

/// Where an actor's model is and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelLocation {
    pub source: ModelSource,
    pub kind: Kind,
}

/// Finds the model of an actor: the player's in the episode's
/// `playerped.rpf`, the others by name in the game's archives — the
/// episode's own first, cutscene models (`cutsprops.img`) before peds,
/// vehicles and map objects; a drawable dictionary (a person) before a
/// fragment before a single drawable.
pub fn find_model(game: &Path, cut_vpath: &str, model: &str) -> Option<ModelLocation> {
    let ep = episode(cut_vpath);
    if model.eq_ignore_ascii_case("player") {
        let rel = format!("{ep}pc/models/cdimages/playerped.rpf");
        let path = rage_rpf::vfs::find_case_insensitive(game, &rel)?;
        let rel = path.strip_prefix(game).ok()?.to_string_lossy().replace('\\', "/");
        return Some(ModelLocation { source: ModelSource::File(format!("{rel}/player.wft")), kind: Kind::Ped });
    }
    let index = crate::gta4::texture_index(game);
    let in_episode = |p: &Path| {
        let rel = p.strip_prefix(game).unwrap_or(p).to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        let ep = ep.to_ascii_lowercase();
        if ep.is_empty() { !rel.starts_with("tlad/") && !rel.starts_with("tbogt/") } else { rel.starts_with(&ep) }
    };
    let archive_rank = |p: &Path| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        ["cutsprops.img", "componentpeds.img", "vehicles.img"].iter().position(|n| name == *n).unwrap_or(3)
    };
    let ext_rank =
        |e: &str| ["wdd", "wft", "wdr"].iter().position(|x| e.to_ascii_lowercase().ends_with(x)).unwrap_or(3);
    let best =
        index.model_files(model).iter().min_by_key(|(p, e)| (!in_episode(p), archive_rank(p), ext_rank(e))).and_then(
            |(p, e)| {
                let rel = p.strip_prefix(game).ok()?.to_string_lossy().replace('\\', "/");
                let kind = if e.to_ascii_lowercase().ends_with(".wdd") {
                    Kind::Ped
                } else if archive_rank(p) == 2 {
                    Kind::Vehicle
                } else {
                    Kind::Prop
                };
                Some(ModelLocation { source: ModelSource::File(format!("{rel}/{e}")), kind })
            },
        );
    if best.is_some() {
        return best;
    }
    // A map object inside a drawable dictionary.
    let near = game.join(ep.trim_end_matches('/'));
    let source = index.model_source(model, &near, game)?;
    Some(ModelLocation { source, kind: Kind::Prop })
}

/// Loads an actor's model, dressed in `outfit` (see [`Gta4Cutscene::outfit`]).
pub fn load_model(
    game: &Path,
    model: &str,
    location: &ModelLocation,
    outfit: &[(&str, u32, u32)],
) -> Result<crate::models::LoadedModel> {
    let mut loaded = load_undressed(game, model, location)?;
    loaded.choose_gta4_components(outfit);
    Ok(loaded)
}

fn load_undressed(game: &Path, model: &str, location: &ModelLocation) -> Result<crate::models::LoadedModel> {
    match &location.source {
        ModelSource::File(vpath) => crate::models::load(game, None, vpath),
        ModelSource::Member { dictionary, hash } => {
            crate::models::load_gta4_members(game, dictionary, &[(model.to_owned(), *hash)])?
                .into_iter()
                .next()
                .map(|(_, m)| m)
                .with_context(|| format!("{model} is not in {dictionary}"))
        }
    }
}

/// A cutscene's animation, its sections played in a row.
pub struct Animated {
    /// Clip `i` is actor `i`'s (of [`Gta4Cutscene::actors`]); the last clip,
    /// `exportcamera`, is the camera. Each clip has one part per section; an
    /// actor's part is an animation without tracks in sections it is not in.
    pub dict: ClipDictionary,
    /// Sections without their animation dictionary and clips not found in it.
    pub problems: Vec<String>,
}

/// Puts the cutscene's animation together from the dictionaries next to the
/// `.cut` at `vpath`.
pub fn animation(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> Result<Animated> {
    let (node, _) = rage_mods::resolve(game, vpath, None)?;
    let Node::Gta4File(archive, id) = node else { bail!("{vpath} is not inside a GTA IV archive") };
    // The folder holding the `.cut` (`.img` archives have only the root).
    let dir = (0..archive.len())
        .find(|&d| archive.entry(d).is_dir() && archive.children(d).contains(&id))
        .unwrap_or(rage_rpf::Gta4Archive::ROOT);
    let name = archive.entry(id).name.clone();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_owned();
    let actors = cut.actors();
    let mut out = ClipDictionary { clips: Vec::new(), animations: vec![Animation::empty()] };
    let mut parts: Vec<Vec<ClipPart>> = vec![Vec::new(); actors.len() + 1];
    let mut problems = Vec::new();
    for (n, section) in cut.sections.iter().enumerate() {
        let file = [format!("{}.wad", section.anim), format!("{stem}_{n}.wad")]
            .into_iter()
            .find_map(|f| archive.find_child(dir, &f));
        let dict = match file {
            Some(f) => Some(rage_formats::wad::parse_wad(&archive.read_resource(f)?)?),
            None => {
                problems.push(format!("section {n}: no {}.wad", section.anim.to_ascii_lowercase()));
                None
            }
        };
        let clip = |name: &str| -> Option<&Animation> {
            let dict = dict.as_ref()?;
            let c = dict.clips.iter().find(|c| c.name.eq_ignore_ascii_case(name))?;
            dict.animations.get(c.parts.first()?.animation)
        };
        let longest = dict.as_ref().map_or(0.0, |d| d.animations.iter().map(|a| a.duration).fold(0.0, f32::max));
        let length = if section.duration > 0.0 { section.duration } else { longest };
        let part = |animation| ClipPart { animation, start: 0.0, end: length, rate: 1.0 };
        // Every section in the first one's frame (a few move to another place).
        let origin = cut.sections[0].offset;
        let shift: [f32; 3] = std::array::from_fn(|k| section.offset[k] - origin[k]);
        for (k, (key, _)) in actors.iter().enumerate() {
            let animation = section.models.iter().find(|m| m.actor() == *key).and_then(|m| {
                let Some(body) = clip(&m.anim) else {
                    if dict.is_some() {
                        problems.push(format!("section {n}: no clip {}", m.anim));
                    }
                    return None;
                };
                let face = m.face.as_deref().and_then(|f| {
                    let found = clip(f);
                    if found.is_none() {
                        problems.push(format!("section {n}: no facial clip {f}"));
                    }
                    found
                });
                Some(actor_animation(body, face, shift))
            });
            let index = match animation {
                Some(a) => {
                    out.animations.push(a);
                    out.animations.len() - 1
                }
                None => 0,
            };
            parts[k].push(part(index));
        }
        let camera = section.camera.as_deref().unwrap_or("camera_0");
        let index = match clip(camera) {
            Some(a) => {
                out.animations.push(actor_animation(a, None, shift));
                out.animations.len() - 1
            }
            None => {
                if dict.is_some() {
                    problems.push(format!("section {n}: no camera clip {camera}"));
                }
                0
            }
        };
        parts[actors.len()].push(part(index));
    }
    let names = actors.iter().map(|(k, _)| k.clone()).chain(std::iter::once("exportcamera".to_owned()));
    out.clips = names.zip(parts).map(|(name, parts)| Clip { name, parts, sequential: true }).collect();
    Ok(Animated { dict: out, problems })
}

/// An actor's (or the camera's) animation in a section: its body clip with
/// the facial clip's tracks added, moved by `shift` (from the section's
/// origin to the first section's). Props move their root bone; that motion
/// becomes the mover (where the model stands), as people's already is.
fn actor_animation(body: &Animation, face: Option<&Animation>, shift: [f32; 3]) -> Animation {
    use rage_formats::clip::{TRACK_BONE_ROTATION, TRACK_BONE_TRANSLATION};
    const MOVER_TRANSLATION: u8 = 5;
    const MOVER_ROTATION: u8 = 6;
    let mut tracks = body.tracks.clone();
    let has_mover = tracks.iter().any(|t| t.bone == 0 && matches!(t.track, MOVER_TRANSLATION | MOVER_ROTATION));
    if !has_mover {
        for t in tracks.iter_mut().filter(|t| t.bone == 0) {
            match t.track {
                TRACK_BONE_TRANSLATION => t.track = MOVER_TRANSLATION,
                TRACK_BONE_ROTATION => t.track = MOVER_ROTATION,
                _ => {}
            }
        }
    }
    let frames = body.frames.max(1);
    let mut values: Vec<Vec<[f32; 4]>> = (0..tracks.len())
        .map(|t| (0..frames).map(|f| body.value(t, f as f32).unwrap_or([0.0, 0.0, 0.0, 1.0])).collect())
        .collect();
    if let Some(face) = face {
        let time = |f: usize| if frames > 1 { f as f32 / (frames - 1) as f32 * body.duration } else { 0.0 };
        for (t, track) in face.tracks.iter().enumerate() {
            if tracks.iter().any(|b| b.bone == track.bone && b.track == track.track) {
                continue;
            }
            tracks.push(*track);
            values.push(
                (0..frames).map(|f| face.value(t, face.frame_at(time(f))).unwrap_or([0.0, 0.0, 0.0, 1.0])).collect(),
            );
        }
    }
    const CAMERA_TRANSLATION: u8 = 7;
    if shift != [0.0; 3] {
        for (t, v) in tracks.iter().zip(&mut values) {
            if t.bone == 0 && matches!(t.track, MOVER_TRANSLATION | CAMERA_TRANSLATION) {
                for p in v.iter_mut() {
                    (0..3).for_each(|k| p[k] += shift[k]);
                }
            }
        }
    }
    Animation::from_frames(frames, body.duration, tracks, values)
}

/// The cutscene's subtitles with their text, from the episode's
/// `american.gxt`: (start, end, text), in seconds.
pub fn subtitles(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> Vec<(f32, f32, String)> {
    if cut.subtitles.is_empty() {
        return Vec::new();
    }
    let rel = format!("{}common/text/american.gxt", episode(vpath));
    let Some(tables) = rage_rpf::vfs::find_case_insensitive(game, &rel)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|d| rage_formats::gxt::parse(&d).ok())
    else {
        return Vec::new();
    };
    // The mission's own table first, then the others.
    let wanted = cut.text_table.as_deref().unwrap_or_default();
    let mut order: Vec<&rage_formats::gxt::Table> = tables.iter().collect();
    order.sort_by_key(|t| !t.name.eq_ignore_ascii_case(wanted));
    cut.subtitles
        .iter()
        .filter_map(|s| {
            let hash = rage_core::hash::joaat(&s.key);
            let text = order.iter().find_map(|t| t.entries.iter().find(|(h, _)| *h == hash).map(|(_, v)| v))?;
            Some((s.start, s.start + s.duration, plain_text(text)))
        })
        .collect()
}

/// Text without the game's markup (`~z~`, `~n~` line breaks, colours).
fn plain_text(text: &str) -> String {
    let mut out = String::new();
    let mut parts = text.split('~');
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    // Every other piece is a tag.
    while let (Some(tag), rest) = (parts.next(), parts.next()) {
        if tag == "n" {
            out.push('\n');
        }
        out.push_str(rest.unwrap_or_default());
    }
    out.trim().to_owned()
}

/// A GTA IV cutscene ready to play: what [`crate::cutscene`] describes for
/// GTA V (length, cast, where it is), its animation, its models and subtitles.
pub struct Scene {
    pub cut: Gta4Cutscene,
    pub cutscene: cutscene::Cutscene,
    pub animated: Animated,
    /// Per actor: where its model is.
    pub models: Vec<Option<ModelLocation>>,
    /// Per actor: what it wears (see [`Gta4Cutscene::outfit`]).
    pub outfits: Vec<Vec<(&'static str, u32, u32)>>,
    /// (start, end, text) in seconds.
    pub subtitles: Vec<(f32, f32, String)>,
}

/// Reads the cutscene at `vpath` and finds its animation, models and subtitles.
pub fn load(game: &Path, vpath: &str) -> Result<Scene> {
    let (node, _) = rage_mods::resolve(game, vpath, None)?;
    let Node::Gta4File(archive, id) = node else { bail!("{vpath} is not inside a GTA IV archive") };
    let cut = parse(&archive.read(id)?)?;
    let animated = animation(game, vpath, &cut)?;
    let actors = cut.actors();
    let models: Vec<Option<ModelLocation>> = actors.iter().map(|(_, m)| find_model(game, vpath, m)).collect();
    let first = cut.sections.first().cloned().unwrap_or_default();
    let cutscene = cutscene::Cutscene {
        lights: Vec::new(),
        duration: cut.duration(),
        // The animations are only moved by the offset; `[ORIENT]` turns
        // the player's start, not the scene.
        offset: first.offset,
        rotation: 0.0,
        actors: actors
            .iter()
            .zip(&models)
            .map(|((_, model), location)| cutscene::Actor {
                kind: location.as_ref().map_or(Kind::Prop, |l| l.kind),
                name: model.clone(),
                object_type: String::new(),
                type_file: None,
            })
            .collect(),
    };
    let subtitles = subtitles(game, vpath, &cut);
    let outfits = actors.iter().map(|(key, _)| cut.outfit(key)).collect();
    Ok(Scene { cut, cutscene, animated, models, outfits, subtitles })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[CUTSCENE_HEADER]\r\n\r\n100\t1272\r\n[/CUTSCENE_HEADER]\r\n\r\n\
[MISSION_TEXT_NAME]\r\n\r\nB1AUD\r\n[/MISSION_TEXT_NAME]\r\n\r\n\
[TEXT]\r\n\r\n204\t696\tB1_CUT1_1\r\n1737\t763\tB1_CUT1_2\r\n[/TEXT]\r\n\r\n\
[FLAGS]\r\n\r\nLONG_FADE_OUT\r\n[/FLAGS]\r\n\r\n\
[PLAYER_START]\r\n\r\n-1153.120972 415.058014 4.570000\r\n[/PLAYER_START]\r\n\r\n\
[VARIATION]\r\n0 4 1 0 0\r\n0 1 0 2 6039\r\n[/VARIATION]\r\n\
[SECTION_START]\r\n\r\n[MODELS]\r\n\r\n\
0 CS_PhilB CS_PhilB_0 cs_philb_head_0\r\n1 player player_0 player_head_0 0\r\n\r\n2 csOldPhone csOldPhone_0\r\n\
[/MODELS]\r\n[LIGHTS]\r\n\r\n[/LIGHTS]\r\n[ORIENT]\r\n\r\n180.000000\r\n[/ORIENT]\r\n\
[REMOVE]\r\n\r\n-1150.6 414.2 5.8 KM_PortacabinDoor\r\n[/REMOVE]\r\n\
[OFFSET]\r\n-1153.120972 415.058014 4.570000\r\n[/OFFSET]\r\n\
[DURATION]\r\n39066.667969\r\n[/DURATION]\r\n[AUDIO]\r\nB_1\r\n[/AUDIO]\r\n\
[ANIM]\r\nB_1_0\r\n[/ANIM]\r\n[CAMERA]\r\ncamera_0\r\n[/CAMERA]\r\n[SECTION_END]\r\n\
[SECTION_START]\r\n[MODELS]\r\n0 csCabaretCurtain csCabaretCurtain-2_1\r\n1 CS_PhilB CS_PhilB_1 cs_philb_head_1\r\n\
[/MODELS]\r\n[VARIATION]\r\n1 0 3 1\r\n[/VARIATION]\r\n[DURATION]\r\n1000\r\n[/DURATION]\r\n[ANIM]\r\nB_1_1\r\n[/ANIM]\r\n[SECTION_END]\r\n";

    #[test]
    fn reads_sections_and_subtitles() {
        let mut data = SAMPLE.as_bytes().to_vec();
        data.extend_from_slice(b"\xcd\xcd\x14\\@\n\0\0\xcd garbage after the text");
        let cut = parse(&data).unwrap();
        assert_eq!(cut.text_table.as_deref(), Some("B1AUD"));
        assert_eq!(cut.subtitles[1], Subtitle { start: 1.737, duration: 0.763, key: "B1_CUT1_2".into() });
        assert_eq!(cut.flags, ["LONG_FADE_OUT"]);
        assert_eq!(cut.player_start, Some([-1153.121, 415.058, 4.57]));
        assert_eq!(cut.sections.len(), 2);
        let s = &cut.sections[0];
        assert_eq!(
            (s.anim.as_str(), s.camera.as_deref(), s.audio.as_deref()),
            ("B_1_0", Some("camera_0"), Some("B_1"))
        );
        assert_eq!((s.orient, s.offset[2]), (180.0, 4.57));
        assert!((s.duration - 39.066_67).abs() < 1e-4);
        assert_eq!(
            s.models[1],
            CutModel { index: 1, model: "player".into(), anim: "player_0".into(), face: Some("player_head_0".into()) }
        );
        assert_eq!(s.models[2].face, None);
        assert_eq!(s.remove, [([-1150.6, 414.2, 5.8], "KM_PortacabinDoor".to_owned())]);
        assert!((cut.duration() - 40.066_67).abs() < 1e-4);
    }

    #[test]
    fn actors_are_the_same_across_sections() {
        let cut = parse(SAMPLE.as_bytes()).unwrap();
        let actors: Vec<String> = cut.actors().into_iter().map(|(k, _)| k).collect();
        assert_eq!(actors, ["cs_philb", "player", "csoldphone", "cscabaretcurtain-2"]);
        assert_eq!(actor_key("camera"), "camera");
    }

    #[test]
    fn reads_outfits() {
        let cut = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(cut.sections[0].variations.len(), 2);
        assert_eq!(cut.sections[0].variations[1].time, 6.039);
        assert_eq!(cut.sections[1].variations[0].model, 1);
        // Phil wears the first section's outfit; the change after six seconds is left out.
        assert_eq!(cut.outfit("cs_philb"), [("hand", 1, 0)]);
        assert!(cut.outfit("player").is_empty());
    }

    #[test]
    fn rejects_broken_files() {
        assert!(parse(b"[TEXT]\n1 2 A\n[/TEXT]\n").is_err(), "no sections");
        assert!(parse(b"[SECTION_START]\n[MODELS]\nCS_PhilB\n[/MODELS]\n[SECTION_END]\n").is_err());
        assert!(parse(b"[SECTION_START]\n[ANIM]\nb_1_0\n[/ANIM]\n").is_err(), "unfinished section");
    }

    #[test]
    fn tolerates_the_game_files_slips() {
        let cut = parse(
            b"[PLAYER_START]\n-1501.92 283.10, 13.78\n[/PLAYER_START]\nVARIATION FOUND!\n\
[TEXT]\n1 2 A\n[/MISSION_TEXT_NAME]\n\
[SECTION_START]\n[MODELS]\n0 a a_0 null\n[/MODELS]\n[SECTION_END]\n",
        )
        .unwrap();
        assert_eq!(cut.player_start, Some([-1501.92, 283.1, 13.78]));
        assert_eq!(cut.subtitles.len(), 1);
        assert_eq!(cut.sections[0].models[0].face, None);
    }

    #[test]
    fn finds_the_episode() {
        assert_eq!(episode("TLAD/pc/anim/cuts.img/x.cut"), "TLAD/");
        assert_eq!(episode("tbogt/pc/anim/cuts.img/x.cut"), "tbogt/");
        assert_eq!(episode("pc/anim/cuts.img/x.cut"), "");
    }

    #[test]
    fn removes_text_markup() {
        assert_eq!(plain_text("~z~Hello, ~r~Niko~s~.~n~Bye"), "Hello, Niko.\nBye");
    }

    #[test]
    fn props_move_by_their_root() {
        use rage_formats::clip::BoneTrack;
        let track = |bone, track, format| BoneTrack { bone, format, track };
        let body = Animation::from_frames(
            2,
            1.0,
            vec![track(0, 0, 0), track(0, 1, 1), track(7, 1, 1)],
            vec![vec![[1.0, 2.0, 3.0, 0.0]; 2], vec![[0.0, 0.0, 0.0, 1.0]; 2], vec![[0.0, 0.0, 0.0, 1.0]; 2]],
        );
        let face = Animation::from_frames(3, 1.0, vec![track(9, 1, 1)], vec![vec![[0.0, 0.0, 1.0, 0.0]; 3]]);
        let a = actor_animation(&body, Some(&face), [0.0; 3]);
        let kinds: Vec<(u16, u8)> = a.tracks.iter().map(|t| (t.bone, t.track)).collect();
        assert_eq!(kinds, [(0, 5), (0, 6), (7, 1), (9, 1)]);
        assert_eq!(a.value(0, 1.0), Some([1.0, 2.0, 3.0, 0.0]));
        assert_eq!(a.value(3, 1.0), Some([0.0, 0.0, 1.0, 0.0]));
        let moved = actor_animation(&body, None, [10.0, 0.0, -1.0]);
        assert_eq!(moved.value(0, 0.0), Some([11.0, 2.0, 2.0, 0.0]), "the mover moves with the section");
        assert_eq!(moved.value(1, 0.0), Some([0.0, 0.0, 0.0, 1.0]));
    }
}
