//! Extraction of archive entries to disk, shared by CLI and GUI.
//!
//! Output layout matches OpenIV: binaries are written decrypted and
//! decompressed, resources as standalone RSC7 files, nested archives as `.rpf`
//! files unless `expand_archives` is set.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use rage_rpf::vfs;
use rage_rpf::{EntryId, Rpf7};

#[derive(Debug, Default, Clone)]
pub struct ExtractSummary {
    /// File or directory that was created.
    pub target: PathBuf,
    pub files: u64,
    pub bytes: u64,
    pub failures: Vec<String>,
    pub cancelled: bool,
}

/// Extracts one entry (file, directory, or nested archive) into `out`.
///
/// `progress` is called after every written file with the running summary and
/// the entry path. Setting `cancel` stops the extraction at the next file.
pub fn extract_entry(
    rpf: &Arc<Rpf7>,
    id: EntryId,
    out: &Path,
    expand_archives: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ExtractSummary, &str),
) -> Result<ExtractSummary> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let entry = rpf.entry(id);
    let own_name = if id == Rpf7::ROOT { rpf.name() } else { &entry.name };
    let own_name = vfs::safe_relative_path(own_name).with_context(|| format!("unsafe entry name {own_name:?}"))?;
    let mut summary = ExtractSummary { target: out.join(&own_name), ..Default::default() };

    if !entry.is_dir() && !(expand_archives && entry.is_archive()) {
        let data = rpf.read(id)?;
        fs::write(&summary.target, &data).with_context(|| format!("writing {}", summary.target.display()))?;
        summary.files = 1;
        summary.bytes = data.len() as u64;
        progress(&summary, &entry.name);
        return Ok(summary);
    }

    let (start, start_dir) =
        if entry.is_archive() { (Arc::new(rpf.open_child(id)?), Rpf7::ROOT) } else { (Arc::clone(rpf), id) };
    let base = summary.target.clone();
    fs::create_dir_all(&base)?;

    let mut visit = |a: &Rpf7, eid: EntryId, rel: &str| {
        if summary.cancelled || cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            return;
        }
        let e = a.entry(eid);
        let Some(target) = vfs::safe_relative_path(rel).map(|r| base.join(r)) else {
            summary.failures.push(format!("{rel}: unsafe path skipped"));
            return;
        };
        let result = if e.is_dir() || (expand_archives && e.is_archive()) {
            fs::create_dir_all(&target).map_err(anyhow::Error::from)
        } else {
            a.read(eid).map_err(anyhow::Error::from).and_then(|data| {
                fs::write(&target, &data)?;
                summary.files += 1;
                summary.bytes += data.len() as u64;
                Ok(())
            })
        };
        match result {
            Ok(()) if !e.is_dir() => progress(&summary, rel),
            Ok(()) => {}
            Err(err) => summary.failures.push(format!("{rel}: {err:#}")),
        }
    };
    let mut nested_errors = Vec::new();
    vfs::walk(&start, start_dir, "", expand_archives, &mut visit, &mut |e| nested_errors.push(e.to_string()));
    summary.failures.extend(nested_errors);
    Ok(summary)
}

/// Human-readable byte size (`1.5M`, `312B`).
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n}B") } else { format!("{v:.1}{}", UNITS[u]) }
}
