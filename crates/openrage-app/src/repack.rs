//! Rewriting an archive with edits, OpenIV style.
//!
//! Unchanged entries are copied byte for byte while writing (streamed, one
//! entry at a time); nested archives are rebuilt (in memory) only if an edit
//! reaches inside them.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_rpf::vfs;
use rage_rpf::{ArchiveWriter, Rpf7};

#[derive(Debug, Clone)]
pub enum Edit {
    /// Put this file content at the path (replacing or adding). RSC7 files become resources.
    Set(Vec<u8>),
    Remove,
}

fn writer_for(rpf: &Arc<Rpf7>, edits: &BTreeMap<String, Edit>) -> Result<ArchiveWriter> {
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
                split = Some((i + 1, child));
                break;
            }
            dir = child;
        }
        match split {
            Some((n, child)) => {
                nested
                    .entry(rpf.path_of(child).to_ascii_lowercase())
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
            match nested.get(&path.to_ascii_lowercase()) {
                Some(inner) => {
                    let result = a.open_child(id).map_err(anyhow::Error::from).and_then(|child| {
                        let bytes = repack(&Arc::new(child), inner)?;
                        w.add_file(path, bytes).map_err(anyhow::Error::from)
                    });
                    if let Err(e) = result {
                        failure = Some(e.context(format!("rebuilding {path}")));
                    }
                }
                None => w.add_copy(path, Arc::clone(rpf), id),
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
    Ok(w)
}

/// Rebuilds `rpf` with `edits` applied, in memory. Paths are relative to the
/// archive and may reach into nested archives (`levels/gta5/vehicles.rpf/adder.ytd`).
pub fn repack(rpf: &Arc<Rpf7>, edits: &BTreeMap<String, Edit>) -> Result<Vec<u8>> {
    Ok(writer_for(rpf, edits)?.build()?)
}

/// Like [`repack`], but streams the result to `out`. Returns the bytes written.
pub fn repack_into<W: Write>(rpf: &Arc<Rpf7>, edits: &BTreeMap<String, Edit>, out: &mut W) -> Result<u64> {
    writer_for(rpf, edits)?.write_to(out).context("writing archive")
}
