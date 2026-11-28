//! Applying a list of [`Op`]s to an open archive editor, including the
//! operations that reach into nested archives.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_rpf::{ArchiveEditor, EntryId, Rpf7, entry_for, file_entry};

use super::Op;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The ops for each nested archive (by path), with the archive's entry.
type NestedOps = BTreeMap<String, (EntryId, Vec<(String, Op)>)>;

/// Applies `ops` to an open editor; ops reaching into nested archives rebuild
/// those (through a temporary file carrying the nested archive's name, which
/// its encryption key depends on).
pub(super) fn apply_ops(
    editor: &mut ArchiveEditor,
    ops: &[(String, Op)],
    keys: &Option<Arc<GtaVKeys>>,
    temp_root: &Path,
) -> Result<()> {
    let (nested, local) = sort_ops(editor.archive(), ops)?;

    for (nested_path, (id, sub_ops)) in nested {
        apply_nested(editor, &nested_path, id, &sub_ops, keys, temp_root)?;
    }

    for (path, op) in local {
        apply_local(editor, path, op)?;
    }
    Ok(())
}

/// Splits `ops` into those for nested archives and those for the archive itself.
fn sort_ops(rpf: &Rpf7, ops: &[(String, Op)]) -> Result<(NestedOps, Vec<(String, Op)>)> {
    let mut nested = NestedOps::new();
    let mut local: Vec<(String, Op)> = Vec::new();
    for (path, op) in ops {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        match find_nested(rpf, &segs) {
            Some((n, id)) => {
                let prefix = segs[..n].join("/");
                let rest = segs[n..].join("/");
                let op = inner_op(op, path, &prefix)?;
                nested.entry(rpf.path_of(id)).or_insert_with(|| (id, Vec::new())).1.push((rest, op));
            }
            None => local.push((segs.join("/"), op.clone())),
        }
    }
    Ok((nested, local))
}

/// The first nested archive on the way down `segs`: how many segments lead to it, and its entry.
fn find_nested(rpf: &Rpf7, segs: &[&str]) -> Option<(usize, EntryId)> {
    let mut dir = Rpf7::ROOT;
    for (i, seg) in segs.iter().enumerate().take(segs.len().saturating_sub(1)) {
        let child = rpf.find_child(dir, seg)?;
        if rpf.entry(child).is_archive() {
            return Some((i + 1, child));
        }
        dir = child;
    }
    None
}

/// `op` as the nested archive (at `prefix`) sees it.
fn inner_op(op: &Op, path: &str, prefix: &str) -> Result<Op> {
    Ok(match op {
        Op::Rename(to) => {
            let to_rest = to
                .strip_prefix(&format!("{prefix}/"))
                .with_context(|| format!("cannot move {path} out of {prefix}"))?;
            Op::Rename(to_rest.to_owned())
        }
        other => other.clone(),
    })
}

fn apply_nested(
    editor: &mut ArchiveEditor,
    nested_path: &str,
    id: EntryId,
    sub_ops: &[(String, Op)],
    keys: &Option<Arc<GtaVKeys>>,
    temp_root: &Path,
) -> Result<()> {
    let rpf = editor.archive();
    let name = rpf.entry(id).name.clone();
    let bytes = rpf.read(id)?;
    let dir = temp_root.join(format!("{}-{}", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&dir)?;
    let tmp = dir.join(&name);
    fs::write(&tmp, bytes)?;
    let result = rebuild_nested(&tmp, sub_ops, keys, temp_root);
    let _ = fs::remove_dir_all(&dir);
    editor.set(nested_path, file_entry(nested_path, result.with_context(|| format!("editing {nested_path}"))?)?)?;
    Ok(())
}

/// Applies `sub_ops` to the nested archive saved at `tmp` and returns its new bytes.
fn rebuild_nested(
    tmp: &Path,
    sub_ops: &[(String, Op)],
    keys: &Option<Arc<GtaVKeys>>,
    temp_root: &Path,
) -> Result<Vec<u8>> {
    let mut sub = ArchiveEditor::open(tmp, keys.clone())?;
    apply_ops(&mut sub, sub_ops, keys, temp_root)?;
    sub.commit(&mut |_| Ok(()))?;
    Ok(fs::read(tmp)?)
}

fn apply_local(editor: &mut ArchiveEditor, path: String, op: Op) -> Result<()> {
    match op {
        Op::Set(data) => editor.set(&path, entry_for(&path, data)?)?,
        Op::SetRaw(entry) => editor.set(&path, entry)?,
        other => return apply_tree_op(editor, &path, other),
    }
    Ok(())
}

/// The ops that change the archive's tree rather than put content at a path.
fn apply_tree_op(editor: &mut ArchiveEditor, path: &str, op: Op) -> Result<()> {
    match op {
        Op::Remove => remove_entry(editor, path)?,
        Op::Rename(to) => editor.rename(path, &to)?,
        Op::CreateDir => editor.create_dir(path)?,
        Op::Set(_) | Op::SetRaw(_) => {}
    }
    Ok(())
}

fn remove_entry(editor: &mut ArchiveEditor, path: &str) -> Result<()> {
    if editor.remove(path) == 0 {
        bail!("{path}: no such entry");
    }
    Ok(())
}
