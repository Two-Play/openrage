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
    /// The text table it is in (`[MISSION_TEXT_NAME]` of its header).
    pub table: Option<String>,
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
    /// The lights' clips (`FillLight1_0`).
    pub lights: Vec<String>,
    /// Accessories of the section's people (`[PROPS]`, before the section or in it).
    pub props: Vec<PropChoice>,
    /// Vehicle colours (`[VEHICLE_DETAILS]`: model, then four numbers of
    /// `carcols.dat`'s colours; all 0 when the cutscene sets none).
    pub vehicles: Vec<(u32, [u32; 4])>,
    /// Particle effects (`[EFFECTS]`).
    pub effects: Vec<CutEffect>,
}

/// One `[EFFECTS]` line: `FX_muz_machine:effect1_0 2 0 40499 40499 x y z
/// rx ry rz 0` — the effect (the part after `:` only tells the effects
/// apart), the model it is attached to (its number in the section's
/// `[MODELS]`, -1 for none), that model's bone (tag, 0 for its root, 1205
/// a person's head), start and end in milliseconds into the section's scene
/// (see [`Gta4Cutscene::scenes`]; the same for a single burst, otherwise the
/// effect runs on until the end),
/// the position in the bone's space and its rotation (radians about X, Y,
/// Z). The number after them is always 0; two cutscenes add a word and a
/// number (`smoke 1.000000`).
#[derive(Debug, Clone, PartialEq)]
pub struct CutEffect {
    /// The effect's name (`FX_muz_machine`).
    pub name: String,
    pub model: Option<u32>,
    pub bone: u16,
    /// Seconds into the section's scene.
    pub start: f32,
    pub end: f32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
}

/// A cutscene's effect with its times in the whole cutscene and the actor
/// (see [`Gta4Cutscene::actors`]) it is attached to.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedEffect {
    pub effect: CutEffect,
    /// The section it belongs to.
    pub section: usize,
    pub actor: Option<usize>,
    /// Seconds into the cutscene.
    pub start: f32,
    pub end: f32,
}

/// One `[PROPS]` line: `model variant anchor` (`1 0 1`: model 1's first
/// glasses, `p_eyes_000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropChoice {
    /// The model's number in the section's `[MODELS]`.
    pub model: u32,
    /// Where it sits: 0 head (hats), 1 eyes (glasses), 2 ears, … 6 left
    /// wrist (watches).
    pub anchor: u32,
    pub variant: u32,
}

/// Prop anchors by number, as the props are named (`p_head_001`).
const ANCHORS: [&str; 9] = ["head", "eyes", "ears", "mouth", "lhand", "rhand", "lwrist", "rwrist", "hip"];

impl PropChoice {
    /// The prop's name in the ped's prop dictionary (`p_eyes_000`).
    pub fn name(&self) -> Option<String> {
        Some(format!("p_{}_{:03}", ANCHORS.get(self.anchor as usize)?, self.variant))
    }

    /// The bone it sits on.
    pub fn bone(&self) -> &'static str {
        match self.anchor {
            4 => "Char_L_Hand",
            5 => "Char_R_Hand",
            6 => "Char_L_Forearm",
            7 => "Char_R_Forearm",
            8 => "Char_Pelvis",
            _ => "Char_Head",
        }
    }
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

/// What a person wears: (component, drawable, texture variant) per part
/// that differs from the first variant (see [`Variation`]).
pub type Outfit = Vec<(&'static str, u32, u32)>;

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
    /// `[CUTSCENE_HEADER]`: one line per scene of the original animation,
    /// the frames (30 a second) where its sections start and the last one
    /// ends (`165 890 1476 2221 2727`: four sections). Effect times count
    /// from a scene's first frame.
    pub scenes: Vec<Vec<f32>>,
    pub sections: Vec<Section>,
}

impl Gta4Cutscene {
    /// Per section: seconds into the cutscene where its scene (see
    /// [`Gta4Cutscene::scenes`]) starts. Without a header that matches the
    /// sections, every section counts as a scene of its own.
    pub fn scene_starts(&self) -> Vec<f32> {
        let mut starts = Vec::with_capacity(self.sections.len());
        let mut at = 0.0;
        let firsts: Vec<bool> = self.scenes.iter().flat_map(|l| (1..l.len()).map(|i| i == 1)).collect();
        let matched = firsts.len() == self.sections.len();
        let mut start = 0.0;
        for (n, s) in self.sections.iter().enumerate() {
            if !matched || firsts[n] {
                start = at;
            }
            starts.push(start);
            at += s.duration;
        }
        starts
    }

    /// Length in seconds: all sections in a row.
    pub fn duration(&self) -> f32 {
        self.sections.iter().map(|s| s.duration).sum()
    }

    /// What an actor (see [`CutModel::actor`]) wears from the start: the
    /// outfit of the first section it is in, (component, drawable, texture).
    /// Changes later on are left out.
    pub fn outfit(&self, actor: &str) -> Outfit {
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

    /// How an actor's outfit changes after the start (see
    /// [`Gta4Cutscene::outfit`]): when (seconds into the cutscene) and the
    /// whole outfit from then on. Later sections may dress it anew, and a
    /// line with a time changes a part partway through: seconds into its
    /// section, or into the cutscene when that is longer than the section.
    pub fn outfit_changes(&self, actor: &str) -> Vec<(f32, Outfit)> {
        let mut events = self.outfit_events(actor);
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut current = self.outfit(actor);
        let mut out: Vec<(f32, Outfit)> = Vec::new();
        for (at, part) in events {
            match current.iter_mut().find(|c| c.0 == part.0) {
                Some(c) if *c == part => continue,
                Some(c) => *c = part,
                None => current.push(part),
            }
            match out.last_mut() {
                Some((t, outfit)) if *t == at => outfit.clone_from(&current),
                _ => out.push((at, current.clone())),
            }
        }
        out
    }

    /// (when, part) of every outfit change of an actor after the start, in any order.
    fn outfit_events(&self, actor: &str) -> Vec<(f32, (&'static str, u32, u32))> {
        let total = self.duration();
        let mut events = Vec::new();
        let (mut start, mut first) = (0.0, true);
        for s in &self.sections {
            if let Some(m) = s.models.iter().find(|m| m.actor() == actor) {
                section_outfit_events(s, m, (start, total), first, &mut events);
                first = false;
            }
            start += s.duration;
        }
        events
    }

    /// A vehicle's colours (numbers in `carcols.dat`), from the first section
    /// it is in that sets them.
    pub fn vehicle_colours(&self, actor: &str) -> Option<[u32; 4]> {
        self.sections.iter().find_map(|s| {
            let m = s.models.iter().find(|m| m.actor() == actor)?;
            s.vehicles.iter().find(|(i, c)| *i == m.index && *c != [0; 4]).map(|(_, c)| *c)
        })
    }

    /// The accessories an actor wears: those of the first section it is in.
    pub fn props(&self, actor: &str) -> Vec<PropChoice> {
        let Some((section, model)) =
            self.sections.iter().find_map(|s| Some((s, s.models.iter().find(|m| m.actor() == actor)?)))
        else {
            return Vec::new();
        };
        section.props.iter().filter(|p| p.model == model.index).copied().collect()
    }

    /// Every section's effects, with cutscene times and actors.
    pub fn effects(&self) -> Vec<PlacedEffect> {
        let actors = self.actors();
        let starts = self.scene_starts();
        let mut out = Vec::new();
        for (n, s) in self.sections.iter().enumerate() {
            let at = starts[n];
            for e in &s.effects {
                let actor = e
                    .model
                    .and_then(|m| s.models.iter().find(|x| x.index == m))
                    .and_then(|m| actors.iter().position(|(key, _)| *key == m.actor()));
                out.push(PlacedEffect {
                    effect: e.clone(),
                    section: n,
                    actor,
                    start: at + e.start,
                    end: at + e.end.max(e.start),
                });
            }
        }
        out
    }

    /// Every light (its clip's name without the section number, lower case)
    /// in the order they first appear.
    pub fn lights(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for l in self.sections.iter().flat_map(|s| &s.lights) {
            let key = actor_key(l);
            if !out.contains(&key) {
                out.push(key);
            }
        }
        out
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
    let mut parser = CutParser::default();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !parser.line(n, line)? {
            break;
        }
    }
    parser.finish()
}

/// What `parse` has read so far.
#[derive(Default)]
struct CutParser {
    cut: Gta4Cutscene,
    section: Option<Section>,
    block: Option<String>,
    /// Outfits and accessories given before the next section.
    pending: Pending,
}

#[derive(Default)]
struct Pending {
    variations: Vec<Variation>,
    props: Vec<PropChoice>,
}

/// A line of a block, for error messages.
struct BlockLine<'a> {
    n: usize,
    name: &'a str,
    text: &'a str,
}

impl BlockLine<'_> {
    fn bad(&self) -> String {
        format!("{}: unexpected [{}] line: {}", line_at(self.n), self.name, self.text)
    }
}

fn line_at(n: usize) -> String {
    format!("line {}", n + 1)
}

impl CutParser {
    /// Reads one line; false when the text ends here.
    fn line(&mut self, n: usize, line: &str) -> Result<bool> {
        if self.tag(n, line)? {
            return Ok(true);
        }
        let Some(name) = self.block.as_deref() else {
            return Ok(!self.is_padding(line));
        };
        let l = BlockLine { n, name, text: line };
        entry(&l, self.section.as_mut(), &mut self.pending, &mut self.cut)?;
        Ok(true)
    }

    /// Handles a line that starts or ends a section or a block.
    fn tag(&mut self, n: usize, line: &str) -> Result<bool> {
        match line {
            "[SECTION_START]" => {
                if self.section.is_some() {
                    bail!("{}: a section starts inside another one", line_at(n));
                }
                // Outfits given before the section are its own.
                self.section = Some(Section {
                    variations: std::mem::take(&mut self.pending.variations),
                    props: std::mem::take(&mut self.pending.props),
                    ..Section::default()
                });
                self.block = None;
            }
            "[SECTION_END]" => {
                let s = self
                    .section
                    .take()
                    .with_context(|| format!("{}: a section ends that never started", line_at(n)))?;
                self.cut.sections.push(s);
                self.block = None;
            }
            _ => return Ok(self.block_tag(line)),
        }
        Ok(true)
    }

    fn block_tag(&mut self, line: &str) -> bool {
        // A closing tag ends the open block, even with another name (a few
        // files close `[TEXT]` with `[/MISSION_TEXT_NAME]`).
        if line.starts_with("[/") && line.ends_with(']') {
            self.block = None;
            return true;
        }
        // A block starts, its `]` sometimes missing (`[MISSION_TEXT_NAME`).
        if let Some(name) = line.strip_prefix('[').map(|l| l.strip_suffix(']').unwrap_or(l))
            && !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            self.block = Some(name.to_owned());
            return true;
        }
        false
    }

    /// Padding after the last section: left-over memory, not text. (Notes
    /// left by the game's tools, like `VARIATION FOUND!`, are not.)
    fn is_padding(&self, line: &str) -> bool {
        self.section.is_none()
            && !self.cut.sections.is_empty()
            && line.chars().any(|c| !(c == '\t' || (' '..='~').contains(&c)))
    }

    fn finish(self) -> Result<Gta4Cutscene> {
        if self.section.is_some() {
            bail!("the last section has no [SECTION_END]");
        }
        if self.cut.sections.is_empty() {
            bail!("no sections ([SECTION_START] … [SECTION_END])");
        }
        Ok(self.cut)
    }
}

fn entry(l: &BlockLine, section: Option<&mut Section>, pending: &mut Pending, cut: &mut Gta4Cutscene) -> Result<()> {
    match l.name {
        "VARIATION" => variation(l, section, pending),
        "PROPS" => props(l, section, pending),
        _ => match section {
            None => cut_entry(l, cut),
            Some(s) => section_entry(l, s),
        },
    }
}

/// A line outside any section (the file's header).
fn cut_entry(l: &BlockLine, cut: &mut Gta4Cutscene) -> Result<()> {
    let line = l.text;
    match l.name {
        "MISSION_TEXT_NAME" => cut.text_table = Some(line.to_owned()),
        "TEXT" => subtitle(l, cut)?,
        "FLAGS" => cut.flags.push(line.to_owned()),
        "CUTSCENE_HEADER" => cut.scenes.push(line.split_whitespace().filter_map(|v| v.parse().ok()).collect()),
        "PLAYER_START" => cut.player_start = Some(floats(line).with_context(|| l.bad())?),
        _ => {}
    }
    Ok(())
}

fn subtitle(l: &BlockLine, cut: &mut Gta4Cutscene) -> Result<()> {
    let f: Vec<&str> = l.text.split_whitespace().collect();
    let [start, length, key] = f.as_slice() else { bail!(l.bad()) };
    let (start, length): (f32, f32) =
        (start.parse().with_context(|| l.bad())?, length.parse().with_context(|| l.bad())?);
    // A cutscene put together from several has a header before
    // each part: its times count from the part's first section.
    let part = cut.sections.iter().map(|s| s.duration).sum::<f32>();
    cut.subtitles.push(Subtitle {
        start: part + start / 1000.0,
        duration: length / 1000.0,
        key: (*key).to_owned(),
        table: cut.text_table.clone(),
    });
    Ok(())
}

/// A line inside a section.
fn section_entry(l: &BlockLine, s: &mut Section) -> Result<()> {
    let line = l.text;
    match l.name {
        "OFFSET" => s.offset = floats(line).with_context(|| l.bad())?,
        "ORIENT" => s.orient = floats::<1>(line).with_context(|| l.bad())?[0],
        "DURATION" => s.duration = floats::<1>(line).with_context(|| l.bad())?[0] / 1000.0,
        "ANIM" => s.anim = line.to_owned(),
        "CAMERA" => s.camera = Some(line.to_owned()),
        "AUDIO" => s.audio = Some(line.to_owned()),
        _ => return section_list_entry(l, s),
    }
    Ok(())
}

fn section_list_entry(l: &BlockLine, s: &mut Section) -> Result<()> {
    match l.name {
        "MODELS" => s.models.push(cut_model(l)?),
        "LIGHTS" => s.lights.push(l.text.to_owned()),
        "EFFECTS" => s.effects.push(parse_effect(l.text).with_context(|| l.bad())?),
        "VEHICLE_DETAILS" => s.vehicles.push(vehicle_details(l)?),
        "REMOVE" => s.remove.push(removed_object(l)?),
        // Draw distances, lights, effects, vehicles, objects to remove, …
        _ => {}
    }
    Ok(())
}

fn removed_object(l: &BlockLine) -> Result<([f32; 3], String)> {
    let (at, model) = l.text.rsplit_once(char::is_whitespace).with_context(|| l.bad())?;
    Ok((floats(at).with_context(|| l.bad())?, model.to_owned()))
}

fn cut_model(l: &BlockLine) -> Result<CutModel> {
    let f: Vec<&str> = l.text.split_whitespace().collect();
    if f.len() < 3 || f[0].parse::<u32>().is_err() {
        bail!(l.bad());
    }
    // A fourth field is the facial clip, unless it is a number (a flag) or `null`.
    let face =
        f.get(3).filter(|v| v.parse::<f32>().is_err() && !v.eq_ignore_ascii_case("null")).map(|v| (*v).to_owned());
    let index = f[0].parse().with_context(|| l.bad())?;
    Ok(CutModel { index, model: f[1].to_owned(), anim: f[2].to_owned(), face })
}

fn vehicle_details(l: &BlockLine) -> Result<(u32, [u32; 4])> {
    let f: Vec<u32> =
        l.text.split_whitespace().map(|v| v.parse()).collect::<Result<_, _>>().with_context(|| l.bad())?;
    let [model, a, b, c, d, ..] = f[..] else { bail!(l.bad()) };
    Ok((model, [a, b, c, d]))
}

fn variation(l: &BlockLine, section: Option<&mut Section>, pending: &mut Pending) -> Result<()> {
    let f: Vec<f32> =
        l.text.split_whitespace().map(|v| v.parse()).collect::<Result<_, _>>().with_context(|| l.bad())?;
    let (&[model, component, drawable, texture], time) = (&f[..f.len().min(4)], f.get(4)) else { bail!(l.bad()) };
    let v = Variation {
        model: model as u32,
        component: component as u32,
        drawable: drawable as u32,
        texture: texture as u32,
        time: time.copied().unwrap_or(0.0) / 1000.0,
    };
    match section {
        Some(s) => s.variations.push(v),
        None => pending.variations.push(v),
    }
    Ok(())
}

fn props(l: &BlockLine, section: Option<&mut Section>, pending: &mut Pending) -> Result<()> {
    let f: Vec<u32> =
        l.text.split_whitespace().map(|v| v.parse()).collect::<Result<_, _>>().with_context(|| l.bad())?;
    let &[model, variant, anchor] = f.as_slice() else { bail!(l.bad()) };
    let p = PropChoice { model, anchor, variant };
    match section {
        Some(s) => s.props.push(p),
        None => pending.props.push(p),
    }
    Ok(())
}

fn parse_effect(line: &str) -> Option<CutEffect> {
    let mut f = line.split_whitespace();
    let name = f.next()?.split(':').next()?.to_owned();
    // Ten numbers; a few lines add more (`… 0 smoke 1.0`), unused.
    let n: Vec<f32> = f.take(10).map(|v| v.parse().ok()).collect::<Option<_>>()?;
    let &[model, bone, start, end, x, y, z, rx, ry, rz] = n.as_slice() else { return None };
    Some(CutEffect {
        name,
        model: (model >= 0.0).then_some(model as u32),
        bone: if bone > 0.0 { bone as u16 } else { 0 },
        start: start / 1000.0,
        end: end / 1000.0,
        position: [x, y, z],
        rotation: [rx, ry, rz],
    })
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
        return player_model(game, ep);
    }
    let index = crate::gta4::texture_index(game);
    let ep_lower = ep.to_ascii_lowercase();
    let best = index
        .model_files(model)
        .iter()
        .min_by_key(|(p, e)| (!in_episode(game, &ep_lower, p), archive_rank(p), ext_rank(e)))
        .and_then(|(p, e)| model_file_location(game, p, e));
    if best.is_some() {
        return best;
    }
    // A map object inside a drawable dictionary.
    let near = game.join(ep.trim_end_matches('/'));
    let source = index.model_source(model, &near, game)?;
    Some(ModelLocation { source, kind: Kind::Prop })
}

fn player_model(game: &Path, ep: &str) -> Option<ModelLocation> {
    let rel = format!("{ep}pc/models/cdimages/playerped.rpf");
    let path = rage_rpf::vfs::find_case_insensitive(game, &rel)?;
    let rel = path.strip_prefix(game).ok()?.to_string_lossy().replace('\\', "/");
    Some(ModelLocation { source: ModelSource::File(format!("{rel}/player.wft")), kind: Kind::Ped })
}

/// The model file `e` inside the archive `p`; its kind follows from the file type and the archive.
fn model_file_location(game: &Path, p: &Path, e: &str) -> Option<ModelLocation> {
    let rel = p.strip_prefix(game).ok()?.to_string_lossy().replace('\\', "/");
    let kind = if e.to_ascii_lowercase().ends_with(".wdd") {
        Kind::Ped
    } else if archive_rank(p) == 2 {
        Kind::Vehicle
    } else {
        Kind::Prop
    };
    Some(ModelLocation { source: ModelSource::File(format!("{rel}/{e}")), kind })
}

/// Outfit changes of the model `m` in section `s` (`span` is its start and the cutscene's length);
/// in the first section of the actor only the changes after the start count (`first`).
fn section_outfit_events(
    s: &Section,
    m: &CutModel,
    span: (f32, f32),
    first: bool,
    events: &mut Vec<(f32, (&'static str, u32, u32))>,
) {
    let (start, total) = span;
    for v in s.variations.iter().filter(|v| v.model == m.index && (!first || v.time > 0.0)) {
        let Some(component) = v.component_name() else { continue };
        let at = if v.time <= s.duration { start + v.time.max(0.0) } else { v.time };
        if at < total {
            events.push((at, (component, v.drawable, v.texture)));
        }
    }
}

/// Whether an archive is in the episode `ep` (lower case, `""` for the base game).
fn in_episode(game: &Path, ep: &str, p: &Path) -> bool {
    let rel = p.strip_prefix(game).unwrap_or(p).to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    if ep.is_empty() { !rel.starts_with("tlad/") && !rel.starts_with("tbogt/") } else { rel.starts_with(ep) }
}

/// Preference among the archives of a model: cutscene props, peds, vehicles, the rest.
fn archive_rank(p: &Path) -> usize {
    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    ["cutsprops.img", "componentpeds.img", "vehicles.img"].iter().position(|n| name == *n).unwrap_or(3)
}

/// Preference among the file types of a model: dictionary, fragment, drawable, the rest.
fn ext_rank(e: &str) -> usize {
    ["wdd", "wft", "wdr"].iter().position(|x| e.to_ascii_lowercase().ends_with(x)).unwrap_or(3)
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
    /// Clip `i` is actor `i`'s (of [`Gta4Cutscene::actors`]); the next,
    /// `exportcamera`, is the camera; the lights' follow (see
    /// [`Animated::lights`]). Each clip has one part per section; an actor's
    /// part is an animation without tracks in sections it is not in.
    pub dict: ClipDictionary,
    /// The lights' clips (of [`Gta4Cutscene::lights`]).
    pub lights: Vec<usize>,
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
    let lights = cut.lights();
    let mut asm = Assembly {
        out: ClipDictionary { clips: Vec::new(), animations: vec![Animation::empty()] },
        parts: vec![Vec::new(); actors.len() + 1 + lights.len()],
        problems: Vec::new(),
    };
    for (n, section) in cut.sections.iter().enumerate() {
        let dict = asm.section_dictionary(&archive, dir, &stem, n, section)?;
        // Every section in the first one's frame (a few move to another place).
        let origin = cut.sections[0].offset;
        let shift: [f32; 3] = std::array::from_fn(|k| section.offset[k] - origin[k]);
        let longest = dict.as_ref().map_or(0.0, |d| d.animations.iter().map(|a| a.duration).fold(0.0, f32::max));
        let length = if section.duration > 0.0 { section.duration } else { longest };
        let at = SectionAnims { n, dict, shift, length };
        asm.add_actors(&at, section, &actors);
        asm.add_camera(&at, section, actors.len());
        asm.add_lights(&at, section, &lights, actors.len() + 1);
    }
    let Assembly { mut out, parts, problems } = asm;
    let names = actors
        .iter()
        .map(|(k, _)| k.clone())
        .chain(std::iter::once("exportcamera".to_owned()))
        .chain(lights.iter().map(|l| format!("light:{l}")));
    out.clips = names.zip(parts).map(|(name, parts)| Clip { name, parts, sequential: true }).collect();
    let lights = (actors.len() + 1..out.clips.len()).collect();
    Ok(Animated { dict: out, lights, problems })
}

/// The animation being put together by [`animation`].
struct Assembly {
    out: ClipDictionary,
    /// Per clip, one part per section.
    parts: Vec<Vec<ClipPart>>,
    problems: Vec<String>,
}

/// One section's animation dictionary and where its clips go.
struct SectionAnims {
    n: usize,
    dict: Option<ClipDictionary>,
    shift: [f32; 3],
    length: f32,
}

impl SectionAnims {
    fn clip(&self, name: &str) -> Option<&Animation> {
        let dict = self.dict.as_ref()?;
        let c = dict.clips.iter().find(|c| c.name.eq_ignore_ascii_case(name))?;
        dict.animations.get(c.parts.first()?.animation)
    }

    fn part(&self, animation: usize) -> ClipPart {
        ClipPart { animation, start: 0.0, end: self.length, rate: 1.0 }
    }
}

impl Assembly {
    /// Reads the section's `.wad`; a missing one is a problem, not an error.
    fn section_dictionary(
        &mut self,
        archive: &rage_rpf::Gta4Archive,
        dir: usize,
        stem: &str,
        n: usize,
        section: &Section,
    ) -> Result<Option<ClipDictionary>> {
        let file = [format!("{}.wad", section.anim), format!("{stem}_{n}.wad")]
            .into_iter()
            .find_map(|f| archive.find_child(dir, &f));
        match file {
            Some(f) => Ok(Some(rage_formats::wad::parse_wad(&archive.read_resource(f)?)?)),
            None => {
                self.problems.push(format!("section {n}: no {}.wad", section.anim.to_ascii_lowercase()));
                Ok(None)
            }
        }
    }

    /// Adds an animation; its index.
    fn push_animation(&mut self, animation: Animation) -> usize {
        self.out.animations.push(animation);
        self.out.animations.len() - 1
    }

    fn add_actors(&mut self, at: &SectionAnims, section: &Section, actors: &[(String, String)]) {
        for (k, (key, _)) in actors.iter().enumerate() {
            let index = match section.models.iter().find(|m| m.actor() == *key) {
                Some(m) => self.actor_clip(at, m),
                None => 0,
            };
            self.parts[k].push(at.part(index));
        }
    }

    /// The index of an actor's animation in the section; 0 (none) when its clip is missing.
    fn actor_clip(&mut self, at: &SectionAnims, m: &CutModel) -> usize {
        let Some(body) = at.clip(&m.anim) else {
            if at.dict.is_some() {
                self.problems.push(format!("section {}: no clip {}", at.n, m.anim));
            }
            return 0;
        };
        let face = m.face.as_deref().and_then(|f| {
            let found = at.clip(f);
            if found.is_none() {
                self.problems.push(format!("section {}: no facial clip {f}", at.n));
            }
            found
        });
        self.push_animation(actor_animation(body, face, at.shift))
    }

    fn add_camera(&mut self, at: &SectionAnims, section: &Section, slot: usize) {
        let camera = section.camera.as_deref().unwrap_or("camera_0");
        let index = self.clip_index(at, camera, "camera clip");
        self.parts[slot].push(at.part(index));
    }

    fn add_lights(&mut self, at: &SectionAnims, section: &Section, lights: &[String], first_slot: usize) {
        for (k, key) in lights.iter().enumerate() {
            let index = match section.lights.iter().find(|l| actor_key(l) == *key) {
                Some(name) => self.clip_index(at, name, "light clip"),
                None => 0,
            };
            self.parts[first_slot + k].push(at.part(index));
        }
    }

    /// The index of the animation made from clip `name`; 0 (none) with a problem noted when it is missing.
    fn clip_index(&mut self, at: &SectionAnims, name: &str, what: &str) -> usize {
        match at.clip(name) {
            Some(a) => self.push_animation(actor_animation(a, None, at.shift)),
            None => {
                if at.dict.is_some() {
                    self.problems.push(format!("section {}: no {what} {name}", at.n));
                }
                0
            }
        }
    }
}

/// An actor's (or the camera's) animation in a section: its body clip with
/// the facial clip's tracks added, moved by `shift` (from the section's
/// origin to the first section's). Props move their root bone; that motion
/// becomes the mover (where the model stands), as people's already is.
fn actor_animation(body: &Animation, face: Option<&Animation>, shift: [f32; 3]) -> Animation {
    let mut tracks = body.tracks.clone();
    let has_mover = tracks.iter().any(|t| t.bone == 0 && matches!(t.track, MOVER_TRANSLATION | MOVER_ROTATION));
    if !has_mover {
        root_motion_to_mover(&mut tracks);
    }
    let frames = body.frames.max(1);
    let mut values: Vec<Vec<[f32; 4]>> = (0..tracks.len())
        .map(|t| (0..frames).map(|f| body.value(t, f as f32).unwrap_or([0.0, 0.0, 0.0, 1.0])).collect())
        .collect();
    if let Some(face) = face {
        add_face_tracks(body, face, &mut tracks, &mut values);
    }
    if shift != [0.0; 3] {
        shift_translations(&tracks, &mut values, shift);
    }
    Animation::from_frames(frames, body.duration, tracks, values)
}

const MOVER_TRANSLATION: u8 = 5;
const MOVER_ROTATION: u8 = 6;
const CAMERA_TRANSLATION: u8 = 7;

/// The root bone's motion becomes the mover's.
fn root_motion_to_mover(tracks: &mut [rage_formats::clip::BoneTrack]) {
    use rage_formats::clip::{TRACK_BONE_ROTATION, TRACK_BONE_TRANSLATION};
    for t in tracks.iter_mut().filter(|t| t.bone == 0) {
        match t.track {
            TRACK_BONE_TRANSLATION => t.track = MOVER_TRANSLATION,
            TRACK_BONE_ROTATION => t.track = MOVER_ROTATION,
            _ => {}
        }
    }
}

/// Adds the facial clip's tracks the body does not have, sampled at the body's frame times.
fn add_face_tracks(
    body: &Animation,
    face: &Animation,
    tracks: &mut Vec<rage_formats::clip::BoneTrack>,
    values: &mut Vec<Vec<[f32; 4]>>,
) {
    let frames = body.frames.max(1);
    let time = |f: usize| if frames > 1 { f as f32 / (frames - 1) as f32 * body.duration } else { 0.0 };
    for (t, track) in face.tracks.iter().enumerate() {
        if tracks.iter().any(|b| b.bone == track.bone && b.track == track.track) {
            continue;
        }
        tracks.push(*track);
        values
            .push((0..frames).map(|f| face.value(t, face.frame_at(time(f))).unwrap_or([0.0, 0.0, 0.0, 1.0])).collect());
    }
}

/// Moves the mover and camera translations by `shift`.
fn shift_translations(tracks: &[rage_formats::clip::BoneTrack], values: &mut [Vec<[f32; 4]>], shift: [f32; 3]) {
    for (t, v) in tracks.iter().zip(values) {
        if t.bone == 0 && matches!(t.track, MOVER_TRANSLATION | CAMERA_TRANSLATION) {
            for p in v.iter_mut() {
                (0..3).for_each(|k| p[k] += shift[k]);
            }
        }
    }
}

/// Loads a person's accessories from its prop dictionary (`<model>_p.wdd`
/// in `pedprops.img`, the episode's own first), each with the bone it sits
/// on. Accessories the dictionary lacks are left out.
pub fn load_props(
    game: &Path,
    cut_vpath: &str,
    model: &str,
    props: &[PropChoice],
) -> Vec<(crate::models::LoadedModel, &'static str)> {
    if props.is_empty() {
        return Vec::new();
    }
    let dictionaries = prop_dictionaries(game, &episode(cut_vpath).to_ascii_lowercase(), model);
    let mut wanted: Vec<(String, u32, &'static str)> = props
        .iter()
        .filter_map(|p| {
            let name = p.name()?;
            Some((name.clone(), rage_core::hash::joaat(&name), p.bone()))
        })
        .collect();
    let mut out = Vec::new();
    // Each accessory from the first dictionary that has it.
    for vpath in dictionaries {
        if wanted.is_empty() {
            break;
        }
        let names: Vec<(String, u32)> = wanted.iter().map(|(n, h, _)| (n.clone(), *h)).collect();
        for (name, m) in crate::models::load_gta4_members(game, &vpath, &names).unwrap_or_default() {
            if let Some(k) = wanted.iter().position(|(n, ..)| *n == name) {
                out.push((m, wanted.remove(k).2));
            }
        }
    }
    out
}

/// The virtual paths of the prop dictionaries to try for a model, best first.
fn prop_dictionaries(game: &Path, ep: &str, model: &str) -> Vec<String> {
    let index = crate::gta4::texture_index(game);
    // `CS_Mallorie_D` (one of her outfits) has the props of `cs_mallorie_p`,
    // `cs_Bernie_CraneC` those of `cs_bernie_crane_p`.
    let mut shorter = vec![model.to_ascii_lowercase()];
    while let Some((s, _)) = shorter.last().and_then(|s| s.rsplit_once('_')) {
        shorter.push(s.to_owned());
    }
    let mut stems: Vec<String> =
        shorter.iter().flat_map(|s| [s.clone(), s[..s.len().saturating_sub(1)].to_owned()]).collect();
    // A cutscene character's dictionary may lack what the in-game one has
    // (`cs_mori_k_p` has no glasses, `ig_mori_k_p` does).
    let in_game: Vec<String> = stems.iter().filter_map(|s| Some(format!("ig_{}", s.strip_prefix("cs_")?))).collect();
    stems.extend(in_game);
    stems
        .iter()
        .filter_map(|stem| {
            index
                .model_files(&format!("{stem}_p"))
                .iter()
                .filter(|(_, e)| e.to_ascii_lowercase().ends_with(".wdd"))
                .min_by_key(|(p, _)| !in_episode(game, ep, p))
                .and_then(|(p, e)| {
                    Some(format!("{}/{e}", p.strip_prefix(game).ok()?.to_string_lossy().replace('\\', "/")))
                })
        })
        .collect()
}

/// Paints a GTA IV vehicle: its `gta_vehicle_paint1` to `…paint3`
/// materials take the colours 1 to 3.
pub fn paint(model: &crate::models::LoadedModel, look: &mut crate::models::Look, colours: &[[f32; 3]; 3]) {
    for (slot, l) in model.materials.iter().zip(look.materials.iter_mut()) {
        for (k, c) in colours.iter().enumerate() {
            if slot.name_hash == rage_core::hash::joaat(&format!("gta_vehicle_paint{}", k + 1)) {
                l.tint = [c[0], c[1], c[2], 1.0];
            }
        }
    }
}

/// A cutscene light at `time` seconds into its clip, `None` while it is
/// off or out of the section. Its root carries it (the mover, see
/// [`actor_animation`]); the other tracks are its own (see
/// [`rage_formats::wad::TRACK_LIGHT_COLOUR`] and the next).
pub fn light_at(dict: &ClipDictionary, clip: &Clip, time: f32) -> Option<cutscene::Light> {
    use rage_formats::wad::{
        TRACK_LIGHT_COLOUR, TRACK_LIGHT_CONE, TRACK_LIGHT_DIRECTION, TRACK_LIGHT_INTENSITY, TRACK_LIGHT_RANGE,
    };
    let track = |t: u8| cutscene::track_at(dict, clip, 0, t, time);
    let intensity = track(TRACK_LIGHT_INTENSITY).map_or(1.0, |v| v[0]);
    if intensity <= 0.0 {
        return None;
    }
    let position = track(5)?;
    let cone = track(TRACK_LIGHT_CONE).map_or(180.0, |v| v[0]);
    let direction = track(TRACK_LIGHT_DIRECTION).map_or([0.0, 0.0, -1.0], |v| [v[0], v[1], v[2]]);
    Some(cutscene::Light {
        position: [position[0], position[1], position[2]],
        direction,
        color: track(TRACK_LIGHT_COLOUR).map_or([1.0; 3], |v| [v[0], v[1], v[2]]),
        intensity,
        range: track(TRACK_LIGHT_RANGE).map_or(5.0, |v| v[0]).max(0.1),
        cone,
        spot: cone > 0.0 && cone < 179.0,
    })
}

/// The cutscene's subtitles with their text, from the episode's
/// `american.gxt`: (start, end, text), in seconds. A part cutscene whose
/// texts the game mostly lacks shows those of the whole cutscene it is part of.
pub fn subtitles(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> Vec<(f32, f32, String)> {
    let own = own_subtitles(game, vpath, cut);
    // Most of its own texts there: a few missing are the game's.
    if own.len() * 2 >= cut.subtitles.len() {
        return own;
    }
    let Some((whole, offset)) = whole_cutscene(game, vpath, cut) else { return own };
    let end = offset + cut.duration();
    let from_whole: Vec<(f32, f32, String)> = own_subtitles(game, vpath, &whole)
        .into_iter()
        .filter(|(start, ..)| (offset..end).contains(start))
        .map(|(start, stop, text)| (start - offset, stop - offset, text))
        .collect();
    if from_whole.len() > own.len() { from_whole } else { own }
}

fn own_subtitles(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> Vec<(f32, f32, String)> {
    if cut.subtitles.is_empty() {
        return Vec::new();
    }
    // The episode's texts, then the base game's and the other episode's (a
    // few cutscenes name texts of another table).
    let mut tables: Vec<rage_formats::gxt::Table> = Vec::new();
    for ep in [episode(vpath), "", "TLAD/", "TBoGT/"] {
        if ep.eq_ignore_ascii_case(episode(vpath)) && !tables.is_empty() {
            continue;
        }
        let read = rage_rpf::vfs::find_case_insensitive(game, &format!("{ep}common/text/american.gxt"))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| rage_formats::gxt::parse(&d).ok());
        tables.extend(read.unwrap_or_default());
    }
    if tables.is_empty() {
        return Vec::new();
    }
    cut.subtitles
        .iter()
        .filter_map(|s| {
            // The subtitle's own table first, then the others.
            let wanted = s.table.as_deref().or(cut.text_table.as_deref()).unwrap_or_default();
            let hash = rage_core::hash::joaat(&s.key);
            let find =
                |t: &rage_formats::gxt::Table| t.entries.iter().find(|(h, _)| *h == hash).map(|(_, v)| v.clone());
            let text = tables
                .iter()
                .filter(|t| t.name.eq_ignore_ascii_case(wanted))
                .find_map(find)
                .or_else(|| tables.iter().find_map(find))?;
            Some((s.start, s.start + s.duration, plain_text(&text)))
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

/// Stereo from a GTA IV stream's channels: the first is the middle
/// (dialogue), the others alternate left and right; mono and stereo stay.
pub fn to_stereo(pcm: &rage_formats::awc::Pcm) -> rage_formats::awc::Pcm {
    let n = pcm.channels.max(1) as usize;
    if n == 2 {
        return pcm.clone();
    }
    let mut samples = Vec::with_capacity(pcm.samples.len() / n * 2);
    for frame in pcm.samples.chunks_exact(n) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        if n == 1 {
            (l, r) = (frame[0] as f32, frame[0] as f32);
        } else {
            let middle = frame[0] as f32 * 0.7;
            for (k, &v) in frame.iter().enumerate().skip(1) {
                if k % 2 == 1 { l += v as f32 } else { r += v as f32 }
            }
            (l, r) = (l + middle, r + middle);
        }
        samples.push(l.clamp(-32768.0, 32767.0) as i16);
        samples.push(r.clamp(-32768.0, 32767.0) as i16);
    }
    rage_formats::awc::Pcm { sample_rate: pcm.sample_rate, channels: 2, samples }
}

/// Where a cutscene sound is: the episode's own sound archive first, then
/// the base game's cutscene sounds.
fn audio_paths(vpath: &str, name: &str) -> Vec<String> {
    let name = name.to_ascii_lowercase();
    let mut out = Vec::new();
    match episode(vpath).to_ascii_lowercase().as_str() {
        "tlad/" => out.push(format!("{}pc/audio/sfx/EP1_SFX.rpf/ep1_sfx/{name}", episode(vpath))),
        "tbogt/" => out.push(format!("{}pc/audio/sfx/EP2_SFX.rpf/ep2_sfx/{name}", episode(vpath))),
        _ => {}
    }
    out.push(format!("pc/audio/sfx/cutscenes.rpf/cutscenes/{name}"));
    out
}

/// The cutscene's sound as one stereo track from its start: a section's
/// `[AUDIO]` starts that sound where the section starts; a section without
/// one (or with the same) lets the sound run on, and so does a sound the
/// game lacks. A part cutscene (`y1_aap2`, `rb03p2`) whose own sound the
/// game lacks plays the whole cutscene's sound (`y1_aa`) from where its
/// first section starts there. Also returns the sounds not found.
pub fn audio(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> (Option<rage_formats::awc::Pcm>, Vec<String>) {
    let (own, missing) = own_audio(game, vpath, cut);
    if own.is_some() {
        return (own, missing);
    }
    match whole_cutscene(game, vpath, cut) {
        Some((whole, offset)) => match own_audio(game, vpath, &whole).0 {
            Some(pcm) => (Some(slice(&pcm, offset, cut.duration())), Vec::new()),
            None => (None, missing),
        },
        None => (None, missing),
    }
}

/// The cutscene `cut` is a part of: another one next to it whose sections
/// include `cut`'s first one (by its animation dictionary), and where that
/// section starts in it.
fn whole_cutscene(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> Option<(Gta4Cutscene, f32)> {
    let first = cut.sections.first()?.anim.to_ascii_lowercase();
    if first.is_empty() {
        return None;
    }
    let (Node::Gta4File(archive, id), _) = rage_mods::resolve(game, vpath, None).ok()? else { return None };
    let dir = (0..archive.len())
        .find(|&d| archive.entry(d).is_dir() && archive.children(d).contains(&id))
        .unwrap_or(rage_rpf::Gta4Archive::ROOT);
    archive
        .children(dir)
        .filter(|&other| other != id && archive.entry(other).name.to_ascii_lowercase().ends_with(".cut"))
        .filter_map(|other| parse(&archive.read(other).ok()?).ok())
        .filter(|whole| whole.sections.len() > cut.sections.len())
        .find_map(|whole| {
            let k = whole.sections.iter().position(|s| s.anim.eq_ignore_ascii_case(&first))?;
            let offset = whole.sections[..k].iter().map(|s| s.duration).sum();
            Some((whole, offset))
        })
}

/// `length` seconds of a stereo track from `start` (silence past its end).
fn slice(pcm: &rage_formats::awc::Pcm, start: f32, length: f32) -> rage_formats::awc::Pcm {
    let rate = pcm.sample_rate as f32;
    let from = ((start * rate) as usize * 2).min(pcm.samples.len());
    let to = (from + (length * rate) as usize * 2).min(pcm.samples.len());
    rage_formats::awc::Pcm { sample_rate: pcm.sample_rate, channels: 2, samples: pcm.samples[from..to].to_vec() }
}

/// A cutscene's own sound (see [`audio`]), without looking for the whole
/// cutscene it may be part of.
fn own_audio(game: &Path, vpath: &str, cut: &Gta4Cutscene) -> (Option<rage_formats::awc::Pcm>, Vec<String>) {
    let segments = audio_segments(cut);
    let total = cut.duration();
    let mut missing: Vec<String> = Vec::new();
    let sounds = decode_sounds(game, vpath, &segments, &mut missing);
    // A sound the game lacks lets the one before run on; so does the same
    // sound named again.
    let mut found: Vec<(f32, &rage_formats::awc::Pcm)> = Vec::new();
    let mut last: Option<&str> = None;
    for (start, name) in &segments {
        let Some(Some(pcm)) = sounds.get(name) else { continue };
        if last != Some(name.as_str()) {
            found.push((*start, pcm));
            last = Some(name);
        }
    }
    let mut out: Option<rage_formats::awc::Pcm> = None;
    for (i, (start, pcm)) in found.iter().enumerate() {
        let end = found.get(i + 1).map_or(f32::MAX, |(s, _)| *s);
        let track = out.get_or_insert_with(|| rage_formats::awc::Pcm {
            sample_rate: pcm.sample_rate,
            channels: 2,
            samples: Vec::new(),
        });
        append_sound(track, pcm, (*start, end), total);
    }
    // Nothing plays past the cutscene's end.
    if let Some(track) = out.as_mut() {
        track.samples.truncate((total * track.sample_rate as f32) as usize * 2);
    }
    (out, missing)
}

/// (start in the cutscene, sound name) of every section that names a sound.
fn audio_segments(cut: &Gta4Cutscene) -> Vec<(f32, String)> {
    let mut segments = Vec::new();
    let mut at = 0.0;
    for s in &cut.sections {
        if let Some(name) = s.audio.as_deref().filter(|n| !n.is_empty()) {
            segments.push((at, name.to_ascii_lowercase()));
        }
        at += s.duration;
    }
    segments
}

/// Decodes each named sound once (stereo); the names the game lacks go to `missing`.
fn decode_sounds(
    game: &Path,
    vpath: &str,
    segments: &[(f32, String)],
    missing: &mut Vec<String>,
) -> std::collections::HashMap<String, Option<rage_formats::awc::Pcm>> {
    let mut sounds: std::collections::HashMap<String, Option<rage_formats::awc::Pcm>> = Default::default();
    for (_, name) in segments {
        sounds.entry(name.clone()).or_insert_with(|| {
            let pcm = audio_paths(vpath, name).into_iter().find_map(|p| {
                let (Node::Gta4File(a, id), _) = rage_mods::resolve(game, &p, None).ok()? else { return None };
                rage_formats::ivaud::decode(&a.read(id).ok()?).ok()
            });
            if pcm.is_none() {
                missing.push(name.clone());
            }
            pcm.map(|p| to_stereo(&p))
        });
    }
    sounds
}

/// Appends silence up to the segment's start (`span.0`), then the sound until the next one starts (`span.1`).
fn append_sound(track: &mut rage_formats::awc::Pcm, pcm: &rage_formats::awc::Pcm, span: (f32, f32), total: f32) {
    let (start, end) = span;
    let rate = track.sample_rate as f32;
    let frame = |t: f32| (t * rate) as usize * 2;
    track.samples.resize(track.samples.len().max(frame(start)), 0);
    track.samples.truncate(frame(start));
    let step = pcm.sample_rate as f32 / rate;
    let frames = ((end.min(total.max(start)) - start).max(0.0) * rate) as usize;
    let have = pcm.samples.len() / 2;
    let take = if end == f32::MAX { have.max(frames) } else { frames };
    for f in 0..take {
        let k = (f as f32 * step) as usize;
        if k >= have {
            break;
        }
        track.samples.extend_from_slice(&pcm.samples[2 * k..2 * k + 2]);
    }
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
    pub outfits: Vec<Outfit>,
    /// Per actor: its accessories (see [`Gta4Cutscene::props`]).
    pub props: Vec<Vec<PropChoice>>,
    /// Per actor: its paint (vehicles; colours 1 to 3 as RGB from 0 to 1).
    pub paints: Vec<Option<[[f32; 3]; 3]>>,
    /// Per actor: its outfit changes after the start (see [`Gta4Cutscene::outfit_changes`]).
    pub outfit_changes: Vec<Vec<(f32, Outfit)>>,
    /// (start, end, text) in seconds.
    pub subtitles: Vec<(f32, f32, String)>,
    /// The sound, stereo, from the cutscene's start.
    pub audio: Option<rage_formats::awc::Pcm>,
    /// Sounds the cutscene names that the game does not have.
    pub missing_audio: Vec<String>,
    /// The particle effects (`[EFFECTS]`) and the effect lists they come from.
    pub effects: Vec<PlacedEffect>,
    pub effect_library: crate::gta4_effects::Library,
    /// Effect lists that could not be read.
    pub effect_problems: Vec<String>,
}

/// What [`crate::cutscene`] describes of a cutscene: its length, offset and cast.
fn cutscene_of(
    cut: &Gta4Cutscene,
    actors: &[(String, String)],
    models: &[Option<ModelLocation>],
) -> cutscene::Cutscene {
    let first = cut.sections.first().cloned().unwrap_or_default();
    cutscene::Cutscene {
        lights: Vec::new(),
        duration: cut.duration(),
        // The animations are only moved by the offset; `[ORIENT]` turns
        // the player's start, not the scene.
        offset: first.offset,
        rotation: 0.0,
        actors: actors
            .iter()
            .zip(models)
            .map(|((_, model), location)| cutscene::Actor {
                kind: location.as_ref().map_or(Kind::Prop, |l| l.kind),
                name: model.clone(),
                object_type: String::new(),
                type_file: None,
            })
            .collect(),
    }
}

/// Per actor: its vehicle paint, when the cutscene sets colours the game knows.
fn car_paints(game: &Path, vpath: &str, cut: &Gta4Cutscene, actors: &[(String, String)]) -> Vec<Option<[[f32; 3]; 3]>> {
    let colours = crate::gta4::car_colours(game, episode(vpath));
    actors
        .iter()
        .map(|(key, _)| {
            let c = cut.vehicle_colours(key)?;
            let rgb = |i: usize| colours.get(c[i] as usize).copied();
            Some([rgb(0)?, rgb(1)?, rgb(2)?])
        })
        .collect()
}

/// Reads the cutscene at `vpath` and finds its animation, models and subtitles.
pub fn load(game: &Path, vpath: &str) -> Result<Scene> {
    let (node, _) = rage_mods::resolve(game, vpath, None)?;
    let Node::Gta4File(archive, id) = node else { bail!("{vpath} is not inside a GTA IV archive") };
    let cut = parse(&archive.read(id)?)?;
    let animated = animation(game, vpath, &cut)?;
    let actors = cut.actors();
    let models: Vec<Option<ModelLocation>> = actors.iter().map(|(_, m)| find_model(game, vpath, m)).collect();
    let cutscene = cutscene_of(&cut, &actors, &models);
    let subtitles = subtitles(game, vpath, &cut);
    let outfits = actors.iter().map(|(key, _)| cut.outfit(key)).collect();
    let outfit_changes = actors.iter().map(|(key, _)| cut.outfit_changes(key)).collect();
    let props = actors.iter().map(|(key, _)| cut.props(key)).collect();
    let paints = car_paints(game, vpath, &cut, &actors);
    let (audio, missing_audio) = audio(game, vpath, &cut);
    let effects = cut.effects();
    let (effect_library, effect_problems) =
        if effects.is_empty() { Default::default() } else { crate::gta4_effects::Library::load(game, episode(vpath)) };
    Ok(Scene {
        effects,
        effect_library,
        effect_problems,
        cut,
        cutscene,
        animated,
        models,
        outfits,
        outfit_changes,
        props,
        paints,
        subtitles,
        audio,
        missing_audio,
    })
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
        assert_eq!(
            cut.subtitles[1],
            Subtitle { start: 1.737, duration: 0.763, key: "B1_CUT1_2".into(), table: Some("B1AUD".into()) }
        );
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
        // Phil wears the first section's outfit from the start; the change comes later.
        assert_eq!(cut.outfit("cs_philb"), [("hand", 1, 0)]);
        assert!(cut.outfit("player").is_empty());
        // After six seconds the upper body changes; the second section dresses Phil anew.
        let second = cut.sections[0].duration;
        assert_eq!(
            cut.outfit_changes("cs_philb"),
            [
                (6.039, vec![("hand", 1, 0), ("uppr", 0, 2)]),
                (second, vec![("hand", 1, 0), ("uppr", 0, 2), ("head", 3, 1)])
            ]
        );
        assert!(cut.outfit_changes("player").is_empty());
    }

    #[test]
    fn reads_props() {
        let cut = parse(
            b"[PROPS]\n0 2 0\n1 0 1\n2 0 6\n[/PROPS]\n[SECTION_START]\n[MODELS]\n0 F_M_PEastEuro_01 F_M_PEastEuro_01_0\n\
1 CS_Dmitri CS_Dmitri_0 cs_dmitri_head_0\n2 CS_Mallorie_D CS_Mallorie_D_0\n[/MODELS]\n[SECTION_END]\n",
        )
        .unwrap();
        let hat = cut.props("f_m_peasteuro_01");
        assert_eq!(hat, [PropChoice { model: 0, anchor: 0, variant: 2 }]);
        assert_eq!((hat[0].name().as_deref(), hat[0].bone()), (Some("p_head_002"), "Char_Head"));
        assert_eq!(cut.props("cs_dmitri")[0].name().as_deref(), Some("p_eyes_000"));
        let watch = cut.props("cs_mallorie_d")[0];
        assert_eq!((watch.name().as_deref(), watch.bone()), (Some("p_lwrist_000"), "Char_L_Forearm"));
    }

    #[test]
    fn reads_vehicle_colours() {
        let cut = parse(
            b"[SECTION_START]\n[MODELS]\n0 Biff Biff_0\n1 Biff Biff-1_0\n[/MODELS]\n\
[VEHICLE_DETAILS]\n0 130 114 37 45 0\n1 0 0 0 0 0\n[/VEHICLE_DETAILS]\n[SECTION_END]\n",
        )
        .unwrap();
        assert_eq!(cut.vehicle_colours("biff"), Some([130, 114, 37, 45]));
        assert_eq!(cut.vehicle_colours("biff-1"), None, "no colours set");
    }

    #[test]
    fn reads_effects() {
        // Sections 0 and 1 are one scene, section 2 another.
        let cut = parse(
            b"[CUTSCENE_HEADER]\n100 160 1060\n10 70\n[/CUTSCENE_HEADER]\n\
[SECTION_START]\n[MODELS]\n0 player player_0\n[/MODELS]\n[DURATION]\n2000\n[/DURATION]\n[SECTION_END]\n\
[SECTION_START]\n[MODELS]\n0 csAk47 csAk47_1\n1 player player_1\n[/MODELS]\n[EFFECTS]\n\
FX_muz_machine:effect1_0 0 0 499 499 0.55 -0.005 0.06 0.0 0.07 0.07 0.000000\n\
FX_CS_GT05_water_tap:effect4_0 -1 -1 22500 26500 0 0 0 0 0 0 0\n\
FX_CS_spit:effect1_0 1 1205 100 100 0 0.1 0 0 0 0 0 smoke 1.000000\n[/EFFECTS]\n\
[DURATION]\n30000\n[/DURATION]\n[SECTION_END]\n\
[SECTION_START]\n[MODELS]\n0 player player_2\n[/MODELS]\n[EFFECTS]\n\
FX_CS_spit:effect1_0 0 1205 500 500 0 0 0 0 0 0 0\n[/EFFECTS]\n[DURATION]\n2000\n[/DURATION]\n[SECTION_END]\n",
        )
        .unwrap();
        assert_eq!(cut.scene_starts(), [0.0, 0.0, 32.0]);
        let e = &cut.sections[1].effects;
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].name.as_str(), e[0].model, e[0].bone), ("FX_muz_machine", Some(0), 0));
        assert_eq!(e[0].position, [0.55, -0.005, 0.06]);
        assert_eq!((e[1].model, e[1].bone, e[1].start, e[1].end), (None, 0, 22.5, 26.5));
        let placed = cut.effects();
        // Times in the whole cutscene (from the scene's start); actors by their clips.
        assert_eq!((placed[0].start, placed[0].actor), (0.499, Some(1)));
        assert_eq!((placed[3].start, placed[3].section), (32.5, 2));
        assert_eq!(placed[1].actor, None);
        assert_eq!((placed[2].actor, placed[2].effect.bone), (Some(0), 1205));
    }

    #[test]
    fn reads_cutscenes_put_together() {
        // Two cutscenes in one: each part's header counts its subtitles from
        // its own first section; one header lacks a `]`.
        let cut = parse(
            b"[MISSION_TEXT_NAME]\nA1AUD\n[/MISSION_TEXT_NAME]\n[TEXT]\n500 1000 A1_1\n[/TEXT]\n\
[SECTION_START]\n[DURATION]\n20000\n[/DURATION]\n[ANIM]\na_0\n[/ANIM]\n[SECTION_END]\n\
[CUTSCENE_HEADER]\n541\t3233\n[/CUTSCENE_HEADER]\n[MISSION_TEXT_NAME\nB2AUD\n[/MISSION_TEXT_NAME]\n\
[TEXT]\n1500 1000 B2_1\n[/TEXT]\nVARIATION FOUND!\n\
[SECTION_START]\n[DURATION]\n10000\n[/DURATION]\n[ANIM]\nb_0\n[/ANIM]\n[SECTION_END]\n\xcd\xcd\xcd\x14\n",
        )
        .unwrap();
        assert_eq!(cut.sections.len(), 2, "the second part is read too");
        let second = &cut.subtitles[1];
        assert_eq!((second.start, second.table.as_deref()), (21.5, Some("B2AUD")));
        assert_eq!(cut.subtitles[0].table.as_deref(), Some("A1AUD"));
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
    fn cuts_a_part_out_of_a_sound() {
        use rage_formats::awc::Pcm;
        // Two frames per second: frames 0..=4.
        let whole = Pcm { sample_rate: 2, channels: 2, samples: vec![0, 0, 1, 1, 2, 2, 3, 3, 4, 4] };
        assert_eq!(slice(&whole, 1.0, 1.0).samples, [2, 2, 3, 3]);
        assert_eq!(slice(&whole, 2.0, 5.0).samples, [4, 4], "stops at the end");
        assert!(slice(&whole, 9.0, 1.0).samples.is_empty());
    }

    #[test]
    fn mixes_streams_to_stereo() {
        use rage_formats::awc::Pcm;
        let three = Pcm { sample_rate: 32000, channels: 3, samples: vec![100, 10, 20, -100, 0, 0] };
        let stereo = to_stereo(&three);
        assert_eq!(stereo.channels, 2);
        assert_eq!(stereo.samples, [80, 90, -70, -70]);
        let mono = Pcm { sample_rate: 8000, channels: 1, samples: vec![5, -5] };
        assert_eq!(to_stereo(&mono).samples, [5, 5, -5, -5]);
        assert_eq!(
            audio_paths("TLAD/pc/anim/cuts.img/ab01_aa.cut", "AB01_AA")[0],
            "TLAD/pc/audio/sfx/EP1_SFX.rpf/ep1_sfx/ab01_aa"
        );
        assert_eq!(audio_paths("pc/anim/cuts.img/b_1.cut", "B_1"), ["pc/audio/sfx/cutscenes.rpf/cutscenes/b_1"]);
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
        let track = |bone, track, format| BoneTrack { bone, format, track, ..Default::default() };
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
