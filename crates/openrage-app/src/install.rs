//! Writing changes into the OpenIV `mods` folder.
//!
//! A change to a file inside a game archive is written the OpenIV way: the
//! top-level archive is copied once to `mods/<same path>` (or the copy already
//! there is used) and then edited in place — new data is appended and the
//! table of contents rewritten, so even multi-gigabyte archives change in
//! milliseconds. Changes inside nested archives rebuild just that nested
//! archive. Every step is recorded in the [journal](crate::journal) and can be
//! undone.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_rpf::{ArchiveEditor, CommitStats, RawEntry, Rpf7, entry_for, file_entry, vfs};

use crate::journal::Journal;

/// A virtual path split into its top-level archive and the path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Archive path relative to the game (and mods) folder, e.g. `x64e.rpf`.
    pub archive: String,
    /// Path inside that archive, e.g. `levels/gta5/vehicles.rpf/adder.ytd`.
    pub inner: String,
}

/// Splits `vpath` at its first `.rpf` file found in the game or mods folder.
/// `mods/…` prefixes are accepted and stripped.
pub fn split(game_dir: &Path, vpath: &str) -> Option<Target> {
    let mods = rage_mods::mods_dir(game_dir);
    let mut segs: Vec<&str> = vpath.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if segs.first().is_some_and(|s| s.eq_ignore_ascii_case(rage_mods::overlay::MODS_FOLDER)) {
        segs.remove(0);
    }
    for n in 1..=segs.len() {
        let prefix = segs[..n].join("/");
        if !prefix.to_ascii_lowercase().ends_with(".rpf") {
            continue;
        }
        let is_file = |root: &Path| vfs::find_case_insensitive(root, &prefix).is_some_and(|p| p.is_file());
        if is_file(game_dir) || mods.as_deref().is_some_and(is_file) {
            return Some(Target { archive: prefix, inner: segs[n..].join("/") });
        }
    }
    None
}

/// One change inside an archive.
#[derive(Debug, Clone)]
pub enum Op {
    /// Put this file content at the path (RSC7 files become resources).
    Set(Vec<u8>),
    /// Put this stored entry at the path.
    SetRaw(RawEntry),
    /// Remove the file or folder.
    Remove,
    /// Rename/move to this path (inside the same archive).
    Rename(String),
    /// Create an empty folder.
    CreateDir,
}

#[derive(Debug, Clone)]
pub struct Applied {
    /// The archive in the mods folder that was changed.
    pub path: PathBuf,
    /// True if the archive was copied into the mods folder first.
    pub created: bool,
    pub stats: CommitStats,
}

pub(crate) fn mods_dir_or_create(game_dir: &Path) -> Result<PathBuf> {
    match rage_mods::mods_dir(game_dir) {
        Some(m) => Ok(m),
        None => {
            let m = game_dir.join(rage_mods::overlay::MODS_FOLDER);
            fs::create_dir_all(&m).with_context(|| format!("creating {}", m.display()))?;
            Ok(m)
        }
    }
}

/// Makes sure `archive` exists in the mods folder (copying it from the game
/// once). Returns its path and whether it was just created.
fn ensure_in_mods(game_dir: &Path, mods: &Path, archive: &str, journal: &Journal) -> Result<(PathBuf, String, bool)> {
    if let Some(p) = vfs::find_case_insensitive(mods, archive).filter(|p| p.is_file()) {
        let rel = p.strip_prefix(mods).unwrap_or(Path::new(archive)).to_string_lossy().replace('\\', "/");
        return Ok((p, rel, false));
    }
    let source = vfs::find_case_insensitive(game_dir, archive)
        .filter(|p| p.is_file())
        .with_context(|| format!("{archive} not found in the game folder"))?;
    // Keep the game's casing so the mod loader finds the copy.
    let rel = source.strip_prefix(game_dir).unwrap_or(Path::new(archive)).to_string_lossy().replace('\\', "/");
    let dest = mods.join(&rel);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("rpf.openrage-tmp");
    // `fs::copy` uses copy_file_range on Linux, which reflinks on btrfs/XFS.
    fs::copy(&source, &tmp).with_context(|| format!("copying {} into the mods folder", source.display()))?;
    fs::rename(&tmp, &dest)?;
    journal.record_created(&rel, &format!("Copied {rel} into the mods folder"))?;
    Ok((dest, rel, true))
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Applies `ops` to an open editor; ops reaching into nested archives rebuild
/// those (through a temporary file carrying the nested archive's name, which
/// its encryption key depends on).
fn apply_ops(
    editor: &mut ArchiveEditor,
    ops: &[(String, Op)],
    keys: &Option<Arc<GtaVKeys>>,
    temp_root: &Path,
) -> Result<()> {
    let mut nested: BTreeMap<String, (rage_rpf::EntryId, Vec<(String, Op)>)> = BTreeMap::new();
    let mut local: Vec<(String, Op)> = Vec::new();
    {
        let rpf = editor.archive();
        for (path, op) in ops {
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
                Some((n, id)) => {
                    let prefix = segs[..n].join("/");
                    let rest = segs[n..].join("/");
                    let op = match op {
                        Op::Rename(to) => {
                            let to_rest = to
                                .strip_prefix(&format!("{prefix}/"))
                                .with_context(|| format!("cannot move {path} out of {prefix}"))?;
                            Op::Rename(to_rest.to_owned())
                        }
                        other => other.clone(),
                    };
                    nested.entry(rpf.path_of(id)).or_insert_with(|| (id, Vec::new())).1.push((rest, op));
                }
                None => local.push((segs.join("/"), op.clone())),
            }
        }
    }

    for (nested_path, (id, sub_ops)) in nested {
        let rpf = editor.archive();
        let name = rpf.entry(id).name.clone();
        let bytes = rpf.read(id)?;
        let dir = temp_root.join(format!("{}-{}", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(&name);
        fs::write(&tmp, bytes)?;
        let result = (|| -> Result<Vec<u8>> {
            let mut sub = ArchiveEditor::open(&tmp, keys.clone())?;
            apply_ops(&mut sub, &sub_ops, keys, temp_root)?;
            sub.commit(&mut |_| Ok(()))?;
            Ok(fs::read(&tmp)?)
        })();
        let _ = fs::remove_dir_all(&dir);
        editor
            .set(&nested_path, file_entry(&nested_path, result.with_context(|| format!("editing {nested_path}"))?)?)?;
    }

    for (path, op) in local {
        match op {
            Op::Set(data) => editor.set(&path, entry_for(&path, data)?)?,
            Op::SetRaw(entry) => editor.set(&path, entry)?,
            Op::Remove => {
                if editor.remove(&path) == 0 {
                    bail!("{path}: no such entry");
                }
            }
            Op::Rename(to) => editor.rename(&path, &to)?,
            Op::CreateDir => editor.create_dir(&path)?,
        }
    }
    Ok(())
}

/// Applies `ops` (paths inside `archive`) to the archive's copy in the mods folder.
pub fn apply(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    archive: &str,
    ops: &[(String, Op)],
    description: &str,
) -> Result<Applied> {
    apply_grouped(game_dir, keys, archive, ops, description, None)
}

/// Like [`apply`], recording the journal entries under `group` (e.g. a package).
pub fn apply_grouped(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    archive: &str,
    ops: &[(String, Op)],
    description: &str,
    group: Option<&str>,
) -> Result<Applied> {
    let mods = mods_dir_or_create(game_dir)?;
    let journal = Journal::new(&mods).grouped(group);
    let (path, rel, created) = ensure_in_mods(game_dir, &mods, archive, &journal)?;
    let mut editor = ArchiveEditor::open(&path, keys.clone())?;
    let temp_root = mods.join(".openrage").join("tmp");
    apply_ops(&mut editor, ops, &keys, &temp_root)?;
    let stats = editor.commit(&mut |backup| {
        journal
            .record_edit(&rel, description, backup)
            .map(|_| ())
            .map_err(|e| rage_rpf::RpfError::Io(std::io::Error::other(e.to_string())))
    })?;
    Ok(Applied { path, created, stats })
}

/// Applies one change to a virtual path (e.g. `x64e.rpf/levels/…/adder.ytd`).
pub fn apply_at(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    vpath: &str,
    op: Op,
    description: &str,
) -> Result<Applied> {
    let Some(target) = split(game_dir, vpath) else { bail!("{vpath} is not inside an archive") };
    apply(game_dir, keys, &target.archive, &[(target.inner, op)], description)
}

/// Creates a new, empty top-level archive in the mods folder (e.g. a new DLC pack).
pub fn create_archive(game_dir: &Path, archive: &str, description: &str, group: Option<&str>) -> Result<PathBuf> {
    let mods = mods_dir_or_create(game_dir)?;
    let dest = mods.join(archive.replace('\\', "/"));
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&dest, rage_rpf::ArchiveWriter::new().build()?)?;
    Journal::new(&mods).grouped(group).record_created(archive, description)?;
    Ok(dest)
}

/// Undoes the newest change in the mods folder.
pub fn undo_last(game_dir: &Path) -> Result<Option<crate::journal::Entry>> {
    let Some(mods) = rage_mods::mods_dir(game_dir) else { return Ok(None) };
    Journal::new(&mods).undo_last()
}
