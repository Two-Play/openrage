//! Metadata files (`.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.cut`, `.pso`, …) as XML.
//!
//! The same extension can hold different encodings: RSC meta resources, PSO
//! (`PSIN`), or plain XML text. All are shown as XML, like CodeWalker exports them.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use rage_crypto::GtaVKeys;
use rage_resource::Resource;
use rage_rpf::vfs::Node;

/// Extensions of files that hold metadata.
pub const EXTENSIONS: &[&str] = &["ymap", "ytyp", "ymt", "ymf", "cut", "pso"];

pub fn is_metadata(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.rsplit_once('.').is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

/// How a metadata file is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// RSC resource with embedded structure descriptions (resource version 2).
    Meta,
    /// Big-endian PSO sections (`PSIN`, `PMAP`, `PSCH`, …).
    Pso,
    /// Binary XML (`RBF0`).
    Rbf,
    /// Already XML text.
    Text,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Self::Meta => "RSC meta",
            Self::Pso => "PSO",
            Self::Rbf => "RBF",
            Self::Text => "XML text",
        }
    }
}

/// A metadata file converted to XML.
#[derive(Debug, Clone)]
pub struct Xml {
    pub text: String,
    pub encoding: Encoding,
    /// The root structure, e.g. `CMapData`.
    pub root: Option<String>,
    /// Parts that could not be read (also marked with `<error>` in the text).
    pub errors: Vec<String>,
}

/// Converts the bytes of a metadata file (as stored in the archive: RSC7, PSO or text).
pub fn from_bytes(data: &[u8]) -> Result<Xml> {
    from_bytes_for(data, false)
}

/// Like [`from_bytes`] for a file from Red Dead Redemption 2 (its PSO pointers differ).
pub fn from_bytes_rdr2(data: &[u8]) -> Result<Xml> {
    from_bytes_for(data, true)
}

fn from_bytes_for(data: &[u8], rdr2: bool) -> Result<Xml> {
    if data.starts_with(b"RSC7") {
        let res = Resource::from_rsc7(data)?;
        if !rage_formats::meta::is_meta(res.version) {
            bail!("resource version {} is not a meta file", res.version);
        }
        return from_resource(&res);
    }
    if rage_formats::pso::is_pso(data) {
        let pso = if rdr2 { rage_formats::pso::parse_rdr2(data)? } else { rage_formats::pso::parse(data)? };
        let (text, errors) = pso.to_xml();
        return Ok(Xml { text, encoding: Encoding::Pso, root: pso.root_type(), errors });
    }
    if rage_formats::rbf::is_rbf(data) {
        let (text, errors, root) = rage_formats::rbf::to_xml(data)?;
        return Ok(Xml { text, encoding: Encoding::Rbf, root: Some(root), errors });
    }
    let start = data.iter().position(|b| !b.is_ascii_whitespace() && *b != 0xEF && *b != 0xBB && *b != 0xBF);
    if start.is_some_and(|s| data[s] == b'<') {
        let text = String::from_utf8_lossy(data).into_owned();
        return Ok(Xml { text, encoding: Encoding::Text, root: None, errors: Vec::new() });
    }
    bail!("unknown metadata encoding")
}

/// Converts a meta resource (already decompressed).
pub fn from_resource(res: &Resource) -> Result<Xml> {
    let meta = rage_formats::meta::parse(res)?;
    let (text, errors) = meta.to_xml();
    Ok(Xml { text, encoding: Encoding::Meta, root: meta.root_type(), errors })
}

/// Converts a metadata file in an RDR2 archive (resource or PSO/RBF) to XML.
pub fn from_rpf8(rpf: &rage_rpf::Rpf8, i: usize) -> Result<Xml> {
    let entry = rpf.entries().and_then(|e| e.get(i)).ok_or_else(|| anyhow::anyhow!("archive table not readable"))?;
    if entry.is_resource {
        let res = rpf.read_resource(entry)?;
        if !rage_formats::meta::is_meta(res.version) {
            bail!("resource version {} is not a meta file", res.version);
        }
        return from_resource(&res);
    }
    from_bytes_rdr2(&rpf.read(entry)?)
}

/// Reads a metadata file (mods included) and converts it to XML.
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<Xml> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys)?;
    let data = match node {
        Node::ArchiveFile(rpf, id) => rpf.read(id)?,
        Node::PlainFile(p) => std::fs::read(p)?,
        Node::Rpf8File(rpf, i) => return from_rpf8(&rpf, i),
        _ => bail!("{vpath} is not a file"),
    };
    from_bytes(&data)
}
