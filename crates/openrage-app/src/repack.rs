//! Rewriting an archive with edits, OpenIV style (experimental; basis of stage 4).
//!
//! Unchanged entries are copied byte for byte; nested archives are rebuilt
//! only if an edit reaches inside them.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use rage_rpf::vfs;
use rage_rpf::{ArchiveWriter, Rpf7};

#[derive(Debug, Clone)]
pub enum Edit {
    /// Put this file content at the path (replacing or adding). RSC7 files become resources.
    Set(Vec<u8>),
    Remove,
}

/// Rebuilds `rpf` with `edits` applied; paths are relative to the archive and
/// may reach into nested archives (`levels/gta5/vehicles.rpf/adder.ytd`).
pub fn repack(rpf: &Rpf7, edits: &BTreeMap<String, Edit>) -> Result<Vec<u8>> {
    // Edits that reach into a nested archive are grouped by that archive.
    let mut nested: BTreeMap<String, BTreeMap<String, Edit>> = BTreeMap::new();
    let mut local: BTreeMap<String, Edit> = BTreeMap::new();
    for (path, edit) in edits {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut dir = Rpf7::ROOT;
        let mut split = None;
        for (i, seg) in segs.iter().enumerate().take(segs.len().saturating_sub(1)) {
            let Some(child) = rpf.find_child(dir, seg) else { break };
            if rpf.entry(child).is_archive() {
                split = Some(i + 1);
                break;
            }
            dir = child;
        }
        match split {
            Some(n) => {
                nested
                    .entry(rpf.path_of(resolve_id(rpf, &segs[..n].join("/"))?))
                    .or_default()
                    .insert(segs[n..].join("/"), edit.clone());
            }
            None => {
                local.insert(segs.join("/"), edit.clone());
            }
        }
    }

    let mut w = ArchiveWriter::new();
    let mut failure = None;
    vfs::walk(
        rpf,
        Rpf7::ROOT,
        "",
        false,
        &mut |a, id, path| {
            if failure.is_some() || a.entry(id).is_dir() {
                return;
            }
            let result = match nested.get(path) {
                Some(inner) => a.open_child(id).map_err(anyhow::Error::from).and_then(|child| {
                    let bytes = repack(&child, inner)?;
                    w.add_file(path, bytes).map_err(anyhow::Error::from)
                }),
                None => a.read_raw_entry(id).map(|raw| w.add_raw(path, raw)).map_err(anyhow::Error::from),
            };
            if let Err(e) = result {
                failure = Some(e.context(format!("copying {path}")));
            }
        },
        &mut |_| {},
    );
    if let Some(e) = failure {
        return Err(e);
    }
    for (path, edit) in &local {
        match edit {
            Edit::Remove => {
                if w.remove(path) == 0 {
                    bail!("{path}: no such entry to remove");
                }
            }
            Edit::Set(data) if data.starts_with(b"RSC7") => w.add_resource_file(path, data.clone())?,
            Edit::Set(data) => w.add_file(path, data.clone())?,
        }
    }
    Ok(w.build()?)
}

fn resolve_id(rpf: &Rpf7, path: &str) -> Result<rage_rpf::EntryId> {
    path.split('/')
        .try_fold(Rpf7::ROOT, |dir, seg| rpf.find_child(dir, seg))
        .with_context(|| format!("{path} not found"))
}
