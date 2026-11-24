//! GTA VI models.
//!
//! A model is stored as its system part and then its graphics part. The
//! archive's table says how long they are. In the recovery view both are cut into zlib streams of 64 KiB. Without the table of contents nothing
//! says where one part ends: the recovery view lists a resource as its first
//! entry (which starts the system part) and what follows it. So the parts
//! are found by trying: every cut between two streams is a possible end of
//! the system part, and the reader says whether the graphics part reaches
//! far enough.

use anyhow::{Context, Result, bail};
use rage_formats::drawable::Drawable;
use rage_formats::drawable6::{self, ModelError};
use rage_rpf::Rpf9;

/// Streams unpack to this many bytes, the last one of a part to fewer.
const STREAM: usize = 0x10000;
/// A system part is at most this long.
const MAX_SYSTEM: usize = 32 << 20;
/// A resource is at most this long, and spread over at most this many entries.
const MAX_RESOURCE: usize = 256 << 20;
const MAX_ENTRIES: usize = 4096;

/// Reads the model that starts at entry `index`: its drawables with their names.
pub fn load(rpf: &Rpf9, index: usize) -> Result<Vec<(String, Drawable)>> {
    let entries = rpf.entries()?;
    let first = entries.get(index).context("no such entry")?;
    let mut data = rpf.read(first)?;
    if drawable6::root(&data).is_none() {
        bail!("this resource is not a model");
    }
    if let Some((system, _)) = rpf.resource_parts(first) {
        let (system, graphics) = data.split_at_checked(system as usize).context("the resource is cut off")?;
        return Ok(named(drawable6::parse(system, graphics)?));
    }
    // Where streams end: the possible ends of the system part.
    let mut cuts: Vec<usize> = (1..=data.len().div_ceil(STREAM)).map(|i| (i * STREAM).min(data.len())).collect();
    let mut last = index;
    loop {
        let mut needs_more = false;
        let mut failure = None;
        for cut in cuts.iter().copied().take_while(|c| *c <= MAX_SYSTEM) {
            match drawable6::parse(&data[..cut], &data[cut..]) {
                Ok(drawables) => return Ok(named(drawables)),
                Err(ModelError::Graphics(_)) => needs_more = true,
                Err(e) => failure = failure.or(Some(e)),
            }
        }
        // The graphics part goes on in the next entry.
        if !needs_more || !Rpf9::follows(&entries, last) || last - index >= MAX_ENTRIES || data.len() >= MAX_RESOURCE {
            match failure {
                Some(e) if !needs_more => bail!(e),
                _ => bail!("the model's vertex and index data was not found after it"),
            }
        }
        last += 1;
        let more = rpf.read(&entries[last])?;
        let start = data.len();
        data.extend_from_slice(&more);
        cuts.extend((1..=more.len().div_ceil(STREAM)).map(|i| start + (i * STREAM).min(more.len())));
    }
}

fn named(drawables: Vec<Drawable>) -> Vec<(String, Drawable)> {
    drawables
        .into_iter()
        .enumerate()
        .map(|(i, d)| (if d.name.is_empty() { format!("drawable {i}") } else { d.name.clone() }, d))
        .collect()
}
