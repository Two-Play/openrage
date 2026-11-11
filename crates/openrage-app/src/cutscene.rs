//! What a cutscene (`.cut`) contains: its length and its cast, read from the
//! cutscene's XML (see [`crate::metadata`]).

use anyhow::{Result, bail};
use rage_formats::clip::{Clip, ClipDictionary};
use rage_formats::math::{self, Mat4};
use rage_rpf::{EntryId, Rpf7};

/// Kinds of cutscene objects worth listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ped,
    Vehicle,
    Prop,
    Camera,
    Light,
    Audio,
    Other,
}

impl Kind {
    fn of(object_type: &str) -> Self {
        let t = object_type.to_ascii_lowercase();
        if t.contains("pedmodel") {
            Self::Ped
        } else if t.contains("vehiclemodel") {
            Self::Vehicle
        } else if t.contains("propmodel") || t.contains("weaponmodel") || t.contains("hiddenmodel") {
            Self::Prop
        } else if t.contains("camera") {
            Self::Camera
        } else if t.contains("light") {
            Self::Light
        } else if t.contains("audio") {
            Self::Audio
        } else {
            Self::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ped => "Person",
            Self::Vehicle => "Vehicle",
            Self::Prop => "Prop",
            Self::Camera => "Camera",
            Self::Light => "Light",
            Self::Audio => "Audio",
            Self::Other => "Other",
        }
    }

    /// Model file extensions to look for, best first.
    pub fn model_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Ped => &[".ydd", ".yft"],
            Self::Vehicle => &[".yft"],
            Self::Prop => &[".ydr", ".yft", ".ydd"],
            _ => &[],
        }
    }
}

/// One object of the cutscene.
#[derive(Debug, Clone)]
pub struct Actor {
    pub kind: Kind,
    /// The model (`csb_bogdan`) or the object's name.
    pub name: String,
    /// The C++ type, e.g. `rage__cutfPedModelObject`.
    pub object_type: String,
    /// For props: the archetype file (`.ytyp`) that defines the model.
    pub type_file: Option<String>,
}

/// A light of the cutscene, in scene coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    /// Reach in metres.
    pub range: f32,
    /// Full cone angle in degrees (spot lights).
    pub cone: f32,
    pub spot: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Cutscene {
    /// The cutscene's lights.
    pub lights: Vec<Light>,
    /// Length in seconds.
    pub duration: f32,
    /// Where the scene's origin is in the world, and its rotation about Z (degrees).
    pub offset: [f32; 3],
    pub rotation: f32,
    pub actors: Vec<Actor>,
}

/// A cutscene's animation: its sections (`name-0.ycd`, `name-1.ycd`, … next
/// to the `.cut`) joined into one clip per actor.
pub fn animation(rpf: &Rpf7, cut: EntryId) -> Result<ClipDictionary> {
    let name = rpf.entry(cut).name.clone();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_owned();
    let dir = rpf.entry(cut).parent.unwrap_or(Rpf7::ROOT);
    let mut sections = Vec::new();
    for i in 0.. {
        let Some(id) = rpf.find_child(dir, &format!("{stem}-{i}.ycd")) else { break };
        sections.push(rage_formats::clip::parse_ycd(&rpf.read_resource(id)?)?);
    }
    if sections.is_empty() {
        bail!("no animation sections ({stem}-0.ycd, …) next to {name}");
    }
    Ok(rage_formats::clip::join_sections(sections))
}

/// The virtual path of a file by its exact name, through the search index
/// (the base game first, then updates and DLC).
pub fn find_file(game_dir: &std::path::Path, name: &str) -> Option<String> {
    let path = crate::index_path(game_dir).filter(|p| p.exists())?;
    let index = rage_index::Index::open(&path).ok()?;
    let results = index.search(name, 200, None).ok()?;
    let mut hits: Vec<String> =
        results.hits.into_iter().filter(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.vpath).collect();
    hits.sort_by_key(|v| (v.starts_with("update"), v.len()));
    hits.into_iter().next()
}

/// A prop's textures: its archetype in the cutscene's archetype file names
/// the texture dictionary.
pub fn prop_textures(
    game_dir: &std::path::Path,
    keys: Option<std::sync::Arc<rage_crypto::GtaVKeys>>,
    type_file: &str,
    archetype: &str,
) -> Option<(String, rage_formats::ytd::TextureDictionary)> {
    let ytyp = find_file(game_dir, &format!("{type_file}.ytyp"))?;
    let xml = crate::metadata::load(game_dir, keys.clone(), &ytyp).ok()?;
    let doc = roxmltree::Document::parse_with_options(
        &xml.text,
        roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() },
    )
    .ok()?;
    let txd = doc
        .descendants()
        .filter(|n| n.has_tag_name("Item"))
        .find(|item| {
            item.children()
                .any(|c| c.has_tag_name("name") && c.text().is_some_and(|t| t.eq_ignore_ascii_case(archetype)))
        })?
        .children()
        .find(|c| c.has_tag_name("textureDictionary"))?
        .text()?
        .trim()
        .to_owned();
    let ytd_name = format!("{txd}.ytd");
    let vpath = find_file(game_dir, &ytd_name)?;
    let (node, _) = rage_mods::resolve(game_dir, &vpath, keys).ok()?;
    let rage_rpf::vfs::Node::ArchiveFile(rpf, id) = node else { return None };
    let dict = rage_formats::ytd::parse(&rpf.read_resource(id).ok()?).ok()?;
    Some((ytd_name, dict))
}

/// Reads the cast and length from a cutscene's XML.
pub fn from_xml(xml: &str) -> Result<Cutscene> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() },
    )?;
    let root = doc.root_element();
    fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        n.children().find(|c| c.has_tag_name(name))
    }
    let duration = child(root, "fTotalDuration").and_then(|d| d.attribute("value")).and_then(|v| v.parse().ok());
    let mut actors = Vec::new();
    if let Some(objects) = child(root, "pCutsceneObjects") {
        for item in objects.children().filter(|c| c.has_tag_name("Item")) {
            let object_type = item.attribute("type").unwrap_or_default().to_owned();
            let kind = Kind::of(&object_type);
            let text = |name: &str| child(item, name).and_then(|n| n.text()).map(str::trim).filter(|t| !t.is_empty());
            let Some(name) = text("StreamingName").or_else(|| text("cName")) else { continue };
            if matches!(kind, Kind::Other) {
                continue;
            }
            let type_file = text("typeFile").map(str::to_owned);
            actors.push(Actor { kind, name: name.to_owned(), object_type, type_file });
        }
    }
    let offset = child(root, "vOffset")
        .and_then(|n| {
            Some([n.attribute("x")?.parse().ok()?, n.attribute("y")?.parse().ok()?, n.attribute("z")?.parse().ok()?])
        })
        .unwrap_or([0.0; 3]);
    let rotation = child(root, "fRotation").and_then(|n| n.attribute("value")?.parse().ok()).unwrap_or(0.0);
    let mut lights = Vec::new();
    if let Some(objects) = child(root, "pCutsceneObjects") {
        let v3 = |n: roxmltree::Node, name: &str| -> Option<[f32; 3]> {
            let c = child(n, name)?;
            Some([c.attribute("x")?.parse().ok()?, c.attribute("y")?.parse().ok()?, c.attribute("z")?.parse().ok()?])
        };
        let f = |n: roxmltree::Node, name: &str| -> Option<f32> { child(n, name)?.attribute("value")?.parse().ok() };
        for item in objects.children().filter(|c| c.has_tag_name("Item")) {
            if Kind::of(item.attribute("type").unwrap_or_default()) != Kind::Light {
                continue;
            }
            let Some(position) = v3(item, "vPosition") else { continue };
            lights.push(Light {
                position,
                direction: v3(item, "vDirection").unwrap_or([0.0, 0.0, -1.0]),
                color: v3(item, "vColour").unwrap_or([1.0; 3]),
                intensity: f(item, "fIntensity").unwrap_or(1.0),
                range: f(item, "fFallOff").unwrap_or(5.0).max(0.1),
                cone: f(item, "fConeAngle").unwrap_or(60.0),
                spot: f(item, "iLightType").is_some_and(|t| t == 2.0),
            });
        }
    }
    Ok(Cutscene { duration: duration.unwrap_or(0.0), offset, rotation, actors, lights })
}

/// The clip of every actor in a joined cutscene animation: the k-th actor
/// with a model name plays `name^k` (`name` for the first), with or without
/// the `_dual` suffix.
pub fn actor_clips(actors: &[Actor], dict: &ClipDictionary) -> Vec<Option<usize>> {
    let split = |clip: &str| -> (String, usize) {
        let lower = clip.to_ascii_lowercase();
        let base = lower.strip_suffix("_dual").unwrap_or(&lower);
        match base.rsplit_once('^') {
            Some((name, n)) if n.chars().all(|c| c.is_ascii_digit()) => (name.to_owned(), n.parse().unwrap_or(0)),
            _ => (base.to_owned(), 0),
        }
    };
    let clips: Vec<(String, usize)> = dict.clips.iter().map(|c| split(&c.name)).collect();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    actors
        .iter()
        .map(|a| {
            let name = a.name.to_ascii_lowercase();
            let k = seen.entry(name.clone()).or_insert(0);
            let found = clips.iter().position(|(n, i)| *n == name && *i == *k);
            *k += 1;
            found
        })
        .collect()
}

/// A track's value on bone `bone` at `time` seconds into a clip.
pub fn track_at(dict: &ClipDictionary, clip: &Clip, bone: u16, track: u8, time: f32) -> Option<[f32; 4]> {
    let (_, part, at) = clip.part_at(time)?;
    let anim = dict.animations.get(part.animation)?;
    let t = anim.tracks.iter().position(|tr| tr.bone == bone && tr.track == track)?;
    anim.value(t, anim.frame_at(at))
}

/// Where an actor stands in the scene: its mover (root motion) tracks.
pub fn mover(dict: &ClipDictionary, clip: &Clip, time: f32) -> Mat4 {
    let t = track_at(dict, clip, 0, 5, time).unwrap_or([0.0; 4]);
    let r = track_at(dict, clip, 0, 6, time).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    math::from_trs([t[0], t[1], t[2]], r, [1.0; 3])
}

/// The cutscene camera at a time: position, rotation (looking along its +Y,
/// Z up) and vertical field of view in degrees.
pub fn camera(dict: &ClipDictionary, clip: &Clip, time: f32) -> Option<([f32; 3], [f32; 4], f32)> {
    let t = track_at(dict, clip, 0, 7, time)?;
    let r = track_at(dict, clip, 0, 8, time)?;
    let fov = track_at(dict, clip, 0, 27, time).map_or(45.0, |f| f[0]);
    Some(([t[0], t[1], t[2]], r, if fov > 1.0 && fov < 170.0 { fov } else { 45.0 }))
}

/// The camera clip of a joined cutscene animation.
pub fn camera_clip(dict: &ClipDictionary) -> Option<usize> {
    dict.clips.iter().position(|c| c.name.eq_ignore_ascii_case("exportcamera"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_cast() {
        let xml = r#"<?xml version="1.0"?>
<rage__cutfCutsceneFile2>
 <fTotalDuration value="128.5" />
 <pCutsceneObjects>
  <Item type="rage__cutfAssetManagerObject"><iObjectId value="0" /></Item>
  <Item type="rage__cutfCameraObject"><cName>exportcamera</cName></Item>
  <Item type="rage__cutfPedModelObject"><cName>hash_1</cName><StreamingName>csb_bogdan</StreamingName></Item>
  <Item type="rage__cutfPropModelObject"><StreamingName>xm_prop_x17_lectern_01</StreamingName></Item>
 </pCutsceneObjects>
</rage__cutfCutsceneFile2>"#;
        let c = from_xml(xml).unwrap();
        assert_eq!(c.duration, 128.5);
        let cast: Vec<(Kind, &str)> = c.actors.iter().map(|a| (a.kind, a.name.as_str())).collect();
        assert_eq!(
            cast,
            vec![(Kind::Camera, "exportcamera"), (Kind::Ped, "csb_bogdan"), (Kind::Prop, "xm_prop_x17_lectern_01")]
        );
    }

    #[test]
    fn assigns_clips_to_actors() {
        use rage_formats::clip::ClipPart;
        let clip = |name: &str| Clip {
            name: name.into(),
            parts: vec![ClipPart { animation: 0, start: 0.0, end: 1.0, rate: 1.0 }],
            sequential: true,
        };
        let dict = ClipDictionary {
            clips: vec![
                clip("mp_m_freemode_01^1_dual"),
                clip("mp_m_freemode_01_dual"),
                clip("prop_x^1"),
                clip("prop_x"),
            ],
            animations: Vec::new(),
        };
        let actor =
            |name: &str| Actor { kind: Kind::Ped, name: name.into(), object_type: String::new(), type_file: None };
        let actors = [actor("mp_m_freemode_01"), actor("mp_m_freemode_01"), actor("prop_x"), actor("missing")];
        assert_eq!(actor_clips(&actors, &dict), vec![Some(1), Some(0), Some(3), None]);
    }
}
