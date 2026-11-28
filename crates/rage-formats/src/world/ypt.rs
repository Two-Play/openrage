//! Particle effect dictionaries (`.ypt`, `rage::ptxFxList`).
//!
//! Root (after the 16-byte resource header): name `0x10`, texture dictionary
//! `0x20`, drawable dictionary `0x30`, particle rules `0x38`, effect rules
//! `0x48`, emitter rules `0x50`. The three rule dictionaries are dictionaries
//! like `.ydd`: name hashes at `0x20` (u16 count at `0x28`), pointers at `0x30`
//! (u16 count at `0x38`). Effect and emitter rules keep their name at `0x20`,
//! particle rules at `0x120`.

use rage_resource::{Resource, ResourceError};

use crate::drawable::{self, Drawable};
use crate::ytd::{self, TextureDictionary};

/// A particle effect dictionary.
#[derive(Debug, Clone, Default)]
pub struct ParticleDictionary {
    pub name: String,
    /// Effects, the names scripts and the game use (`bul_gravel_heli`).
    pub effects: Vec<String>,
    /// Emitter rules (where and how particles are spawned).
    pub emitters: Vec<String>,
    /// Particle rules (how single particles look and behave).
    pub particles: Vec<String>,
    pub textures: Option<TextureDictionary>,
    /// Models used by particles (debris, sparks, …) with their name hashes.
    pub drawables: Vec<(u32, Drawable)>,
    /// Parts that could not be read.
    pub errors: Vec<String>,
}

fn names(res: &Resource, dict: u64, name_at: u64, errors: &mut Vec<String>, what: &str) -> Vec<String> {
    if dict == 0 {
        return Vec::new();
    }
    let read = || -> Result<Vec<String>, ResourceError> {
        let list = res.u64(dict + 0x30)?;
        let count = res.u16(dict + 0x38)? as u64;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let rule = res.u64(list + i * 8)?;
            let ptr = res.u64(rule + name_at)?;
            out.push(if ptr != 0 { res.string(ptr)? } else { String::new() });
        }
        Ok(out)
    };
    read().unwrap_or_else(|e| {
        errors.push(format!("{what}: {e}"));
        Vec::new()
    })
}

/// Texture dictionaries inside `.ypt` follow the game's layout (Legacy or
/// Enhanced) while the file version is the same, so the layout that reads
/// cleanly wins.
fn textures(res: &Resource, at: u64) -> Result<Option<TextureDictionary>, ResourceError> {
    if at == 0 || res.u16(at + 0x38)? == 0 {
        return Ok(None);
    }
    let enhanced = ytd::parse_at(res, at, true);
    if let Ok(d) = &enhanced
        && d.errors.is_empty()
    {
        return enhanced.map(Some);
    }
    match ytd::parse_at(res, at, false) {
        Ok(d) if d.errors.is_empty() => Ok(Some(d)),
        _ => enhanced.map(Some),
    }
}

fn drawables(res: &Resource, dict: u64, enhanced: bool) -> Result<Vec<(u32, Drawable)>, ResourceError> {
    if dict == 0 {
        return Ok(Vec::new());
    }
    let hashes = res.u64(dict + 0x20)?;
    let list = res.u64(dict + 0x30)?;
    let count = res.u16(dict + 0x38)? as u64;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count.min(4096) {
        let hash = if hashes != 0 { res.u32(hashes + i * 4)? } else { 0 };
        out.push((hash, drawable::read_drawable(res, res.u64(list + i * 8)?, enhanced)?));
    }
    Ok(out)
}

pub fn parse(res: &Resource) -> Result<ParticleDictionary, ResourceError> {
    let root = Resource::ROOT;
    let name_ptr = res.u64(root + 0x10)?;
    let mut out = ParticleDictionary {
        name: if name_ptr != 0 { res.string(name_ptr)? } else { String::new() },
        ..Default::default()
    };
    let mut errors = Vec::new();
    out.effects = names(res, res.u64(root + 0x48)?, 0x20, &mut errors, "effects");
    out.emitters = names(res, res.u64(root + 0x50)?, 0x20, &mut errors, "emitters");
    out.particles = names(res, res.u64(root + 0x38)?, 0x120, &mut errors, "particle rules");
    match textures(res, res.u64(root + 0x20)?) {
        Ok(t) => out.textures = t,
        Err(e) => errors.push(format!("textures: {e}")),
    }
    // The models use the same layout as the textures (Enhanced or Legacy).
    let enhanced = out.textures.as_ref().is_none_or(|t| t.layout == ytd::Layout::Enhanced);
    let dict = res.u64(root + 0x30)?;
    match drawables(res, dict, enhanced).or_else(|_| drawables(res, dict, !enhanced)) {
        Ok(d) => out.drawables = d,
        Err(e) => errors.push(format!("models: {e}")),
    }
    out.errors = errors;
    Ok(out)
}
