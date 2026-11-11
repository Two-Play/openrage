//! What a cutscene (`.cut`) contains: its length and its cast, read from the
//! cutscene's XML (see [`crate::metadata`]).

use anyhow::{Result, bail};
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
}

#[derive(Debug, Clone, Default)]
pub struct Cutscene {
    /// Length in seconds.
    pub duration: f32,
    pub actors: Vec<Actor>,
}

/// A cutscene's animation: its sections (`name-0.ycd`, `name-1.ycd`, … next
/// to the `.cut`) joined into one clip per actor.
pub fn animation(rpf: &Rpf7, cut: EntryId) -> Result<rage_formats::clip::ClipDictionary> {
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
            actors.push(Actor { kind, name: name.to_owned(), object_type });
        }
    }
    Ok(Cutscene { duration: duration.unwrap_or(0.0), actors })
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
}
