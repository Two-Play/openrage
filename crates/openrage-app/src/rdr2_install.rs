//! Writing Red Dead Redemption 2 changes into the `lml/replace` folder.
//!
//! RDR2's archives are never changed. Lenny's Mod Loader (LML) loads
//! `lml/replace/<archive>/<path>` in place of the file at `<path>` inside
//! the game's `<archive>.rpf`, so:
//!
//! - a file directly inside a game archive is written there as a loose file
//!   (`update_4.rpf/x64/data/foo.ymt` → `lml/replace/update_4/x64/data/foo.ymt`);
//! - a change inside a nested archive (`update_4.rpf/x64/data/ui/startup_data.rpf/…`)
//!   copies that nested archive there once
//!   (`lml/replace/update_4/x64/data/ui/startup_data.rpf`) and edits the copy
//!   in place with the RPF8 writer; archives nested deeper are rebuilt inside it.
//!
//! LML cannot hide or rename a file of the game's archives, so removing and
//! renaming work only inside a nested archive (or on files OpenRAGE put into
//! `lml/replace` itself). Every step is recorded in the journal
//! (`lml/replace/.openrage/journal`) and can be undone.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use rage_mods::overlay::{rdr2_rel, rdr2_split};
use rage_rpf::rpf8::{Rdr2Keys, Rpf8};
use rage_rpf::rpf8_editor::Rpf8Editor;
use rage_rpf::rpf8_names::NameIndex;
use rage_rpf::{CommitStats, vfs};

use crate::install::{Applied, Op};
use crate::journal::Journal;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The nested archive `inner` passes through in `rpf`, as (its path, the
/// rest of `inner`): the shortest prefix that is an archive entry.
fn nested_split(rpf: &Rpf8, inner: &str) -> Option<(String, String)> {
    let segs: Vec<&str> = inner.split('/').filter(|s| !s.is_empty()).collect();
    for n in 1..segs.len() {
        let prefix = segs[..n].join("/");
        if let Some(i) = rpf.find(&prefix)
            && rpf.entries()?[i].ext_id == 0
        {
            return Some((rpf.entry_name(i), segs[n..].join("/")));
        }
    }
    None
}

/// Groups ops by the nested archive they reach into; ops on the archive's
/// own entries come back under `None`.
#[allow(clippy::type_complexity)]
fn group(rpf: &Rpf8, ops: &[(String, Op)]) -> Result<(Vec<(String, Op)>, BTreeMap<String, Vec<(String, Op)>>)> {
    let mut local = Vec::new();
    let mut nested: BTreeMap<String, Vec<(String, Op)>> = BTreeMap::new();
    for (path, op) in ops {
        let path = path.trim_matches('/');
        match nested_split(rpf, path) {
            Some((archive, rest)) => {
                let op = match op {
                    Op::Rename(to) => {
                        let to = to.trim_matches('/');
                        let rest_to = to
                            .get(archive.len() + 1..)
                            .filter(|_| to.len() > archive.len() && to[..archive.len()].eq_ignore_ascii_case(&archive))
                            .with_context(|| format!("cannot move {path} out of {archive}"))?;
                        Op::Rename(rest_to.to_owned())
                    }
                    other => other.clone(),
                };
                nested.entry(archive).or_default().push((rest, op));
            }
            None => local.push((path.to_owned(), op.clone())),
        }
    }
    Ok((local, nested))
}

/// Applies ops to an archive opened for editing; deeper nested archives are
/// rebuilt through temporary files.
fn edit(
    editor: &mut Rpf8Editor,
    ops: &[(String, Op)],
    keys: &Option<Arc<Rdr2Keys>>,
    names: &Option<Arc<NameIndex>>,
    temp_root: &Path,
    added: &mut Vec<String>,
) -> Result<()> {
    let (local, nested) = group(editor.archive(), ops)?;
    for (archive, sub_ops) in nested {
        let rpf = editor.archive();
        let entry = &rpf.entries().context("entry table")?[rpf.find(&archive).context("nested archive")?];
        let bytes = rpf.read(entry)?;
        let dir = temp_root.join(format!("{}-{}", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&dir)?;
        let tmp = dir.join("nested.rpf");
        fs::write(&tmp, bytes)?;
        let result = (|| -> Result<Vec<u8>> {
            let mut sub = Rpf8Editor::open(&tmp, keys.clone(), names.clone())?;
            edit(&mut sub, &sub_ops, keys, names, temp_root, added)?;
            added.extend(sub.added_names().iter().cloned());
            sub.commit(&mut |_| Ok(()))?;
            Ok(fs::read(&tmp)?)
        })();
        let _ = fs::remove_dir_all(&dir);
        editor.set(&archive, result.with_context(|| format!("editing {archive}"))?)?;
    }
    for (path, op) in local {
        match op {
            Op::Set(data) => editor.set(&path, data)?,
            Op::SetRaw(_) => bail!("{path}: stored GTA V entries cannot go into an RDR2 archive"),
            Op::Remove => {
                if editor.remove(&path) == 0 {
                    bail!("{path}: no such entry");
                }
            }
            Op::Rename(to) => editor.rename(&path, &to)?,
            Op::CreateDir => bail!("{path}: RDR2 archives have no folders of their own; import a file there instead"),
        }
    }
    added.extend(editor.added_names().iter().cloned());
    Ok(())
}

/// Applies `ops` (paths inside the game archive `archive`) for RDR2.
pub(crate) fn apply(
    game_dir: &Path,
    mods: &Path,
    archive: &str,
    ops: &[(String, Op)],
    description: &str,
    journal: &Journal,
) -> Result<Applied> {
    let Some((archive, _)) = rdr2_split(game_dir, archive) else {
        bail!("{archive} is not one of the game's archives (RDR2 changes go into lml/replace, beside the game's own)")
    };
    let keys = rage_rpf::rpf8::default_keys();
    let names = rage_rpf::rpf8::default_names();
    let top = Rpf8::open_default(&game_dir.join(&archive))?;
    if top.entries().is_none() {
        bail!("{archive}: the archive keys are not loaded (run `openrage rdr2-keys` first)");
    }
    let (local, nested) = group(&top, ops)?;
    let mut last: Option<Applied> = None;
    let mut added = Vec::new();

    for (path, op) in local {
        check_named(&archive, &path)?;
        let rel = rdr2_rel(&archive, &path);
        let target = mods.join(vfs::safe_relative_path(&rel).with_context(|| format!("{rel}: not a valid path"))?);
        match op {
            Op::Set(data) => {
                check_kind(&top, &path, &data)?;
                let written = journal.write_file(false, &rel, &data, description)?;
                last = Some(Applied { path: written, created: false, stats: changed() });
            }
            Op::Remove if vfs::find_case_insensitive(mods, &rel).is_some_and(|p| p.is_file()) => {
                journal.delete_file(&rel, description)?;
                last = Some(Applied { path: target, created: false, stats: changed() });
            }
            Op::Remove => bail!(
                "{path}: Lenny's Mod Loader cannot remove a file from the game's archives; only files in \
                 lml/replace (and entries of nested archives) can be removed"
            ),
            Op::Rename(_) => bail!(
                "{path}: Lenny's Mod Loader cannot rename a file of the game's archives; only entries of \
                 nested archives can be renamed"
            ),
            Op::CreateDir => bail!("{path}: RDR2 archives have no folders of their own; import a file there instead"),
            Op::SetRaw(_) => bail!("{path}: stored GTA V entries cannot go into an RDR2 archive"),
        }
    }

    let temp_root = mods.join(rage_mods::overlay::OPENRAGE_FOLDER).join("tmp");
    for (nested_path, sub_ops) in nested {
        check_named(&archive, &nested_path)?;
        let rel = rdr2_rel(&archive, &nested_path);
        let (path, created) = ensure_copied(&top, &nested_path, mods, &rel, journal)?;
        let mut editor = Rpf8Editor::open(&path, keys.clone(), names.clone())?;
        edit(&mut editor, &sub_ops, &keys, &names, &temp_root, &mut added)?;
        let stats = editor.commit(&mut |backup| {
            journal
                .record_edit(&rel, description, backup)
                .map(|_| ())
                .map_err(|e| rage_rpf::RpfError::Io(std::io::Error::other(e.to_string())))
        })?;
        last = Some(Applied { path, created, stats });
    }
    remember_names(mods, top.platform(), &added)?;
    last.context("nothing to do")
}

/// LML finds a replacement by the file's real path: refuse paths that are
/// only a hash (`hash/…`) or carry a mount prefix (`pack:/…`).
fn check_named(archive: &str, path: &str) -> Result<()> {
    if path.split('/').next().is_some_and(|s| s.eq_ignore_ascii_case("hash")) || path.contains(':') {
        bail!(
            "{archive}/{path}: the real path of this file is not known, and Lenny's Mod Loader finds replacements \
             by path. Run `openrage rdr2-names <game folder>` (or import a list of names) so the file gets its name"
        );
    }
    Ok(())
}

fn changed() -> CommitStats {
    CommitStats { changed: true, ..Default::default() }
}

/// Refuses to put a plain file where the game has a resource (or the other
/// way round): LML would hand the game data it cannot read.
fn check_kind(top: &Rpf8, path: &str, data: &[u8]) -> Result<()> {
    let Some(i) = top.find(path) else { return Ok(()) };
    let entry = &top.entries().context("entry table")?[i];
    let is_resource = rage_rpf::rpf8::resource_file_flags(data).is_some();
    if entry.is_resource && !is_resource {
        bail!("{path} is a resource: replace it with an RSC8 file (as Extract saves it)");
    }
    if !entry.is_resource && is_resource {
        bail!("{path} is a plain file in the game, the new data is a resource (RSC8 file)");
    }
    Ok(())
}

/// Copies a nested archive out of the game archive into `lml/replace` once.
fn ensure_copied(top: &Rpf8, nested_path: &str, mods: &Path, rel: &str, journal: &Journal) -> Result<(PathBuf, bool)> {
    if let Some(p) = vfs::find_case_insensitive(mods, rel).filter(|p| p.is_file()) {
        return Ok((p, false));
    }
    let i = top.find(nested_path).with_context(|| format!("{nested_path} not found"))?;
    let bytes = top.read(&top.entries().context("entry table")?[i])?;
    let dest = mods.join(vfs::safe_relative_path(rel).with_context(|| format!("{rel}: not a valid path"))?);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("rpf.openrage-tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &dest)?;
    journal.record_created(rel, &format!("Copied {nested_path} into lml/replace"))?;
    Ok((dest, true))
}

/// The names OpenRAGE gave new or renamed entries, kept beside the journal
/// (`lml/replace/.openrage/names.txt`): RPF8 stores only hashes, and the
/// game's own name tables cannot be written.
pub(crate) fn names_file(mods: &Path) -> PathBuf {
    mods.join(rage_mods::overlay::OPENRAGE_FOLDER).join("names.txt")
}

fn remember_names(mods: &Path, platform: char, added: &[String]) -> Result<()> {
    if added.is_empty() {
        return Ok(());
    }
    let file = names_file(mods);
    let mut text = fs::read_to_string(&file).unwrap_or_default();
    for n in added {
        if !text.lines().any(|l| l.eq_ignore_ascii_case(n)) {
            text.push_str(n);
            text.push('\n');
        }
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
    // Known from now on in this process, too.
    let mut index = rage_rpf::rpf8::default_names().map(|n| (*n).clone()).unwrap_or_default();
    for n in added {
        index.add_path(n, platform);
    }
    rage_rpf::rpf8::set_defaults(rage_rpf::rpf8::default_keys(), Some(Arc::new(index)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install;

    fn archive(at: &Path, files: &[(&str, Vec<u8>)]) {
        fs::write(at, rage_rpf::rpf8_editor::empty_archive()).unwrap();
        let mut ed = Rpf8Editor::open(at, None, None).unwrap();
        for (p, d) in files {
            ed.set(p, d.clone()).unwrap();
        }
        ed.commit(&mut |_| Ok(())).unwrap();
    }

    fn read(game: &Path, vpath: &str) -> Vec<u8> {
        match rage_mods::resolve(game, vpath, None).unwrap().0 {
            vfs::Node::Rpf8File(rpf, i) => rpf.read(&rpf.entries().unwrap()[i]).unwrap(),
            vfs::Node::PlainFile(p) => fs::read(p).unwrap(),
            _ => panic!("{vpath} is not a file"),
        }
    }

    #[test]
    fn changes_go_into_lml_replace_and_undo() {
        let game = std::env::temp_dir().join(format!("openrage-rdr2-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&game);
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("RDR2.exe"), b"").unwrap();
        let nested = game.join("nested.tmp");
        archive(&nested, &[("settings/a.ymt", b"inner a".to_vec()), ("settings/b.ymt", b"inner b".to_vec())]);
        let nested_bytes = fs::read(&nested).unwrap();
        fs::remove_file(&nested).unwrap();
        archive(
            &game.join("update_9.rpf"),
            &[("x64/data/top.ymt", b"top".to_vec()), ("x64/data/ui/inner.rpf", nested_bytes.clone())],
        );
        let before = fs::read(game.join("update_9.rpf")).unwrap();

        // A file directly in the archive: a loose file in lml/replace.
        install::apply_at(&game, None, "update_9.rpf/x64/data/top.ymt", Op::Set(b"TOP".to_vec()), "top").unwrap();
        let loose = game.join("lml/replace/update_9/x64/data/top.ymt");
        assert_eq!(fs::read(&loose).unwrap(), b"TOP");
        assert_eq!(read(&game, "update_9.rpf/x64/data/top.ymt"), b"TOP", "the overlay view sees it");
        assert!(
            install::apply_at(&game, None, "update_9.rpf/x64/data/ui/inner.rpf", Op::Rename("x.rpf".into()), "mv")
                .is_err(),
            "LML cannot rename the game's files"
        );

        // Inside the nested archive: a copy of it in lml/replace, edited.
        let inner = "update_9.rpf/x64/data/ui/inner.rpf/settings";
        let ops = [
            ("x64/data/ui/inner.rpf/settings/a.ymt".to_owned(), Op::Set(b"changed a".to_vec())),
            ("x64/data/ui/inner.rpf/settings/new.ymt".to_owned(), Op::Set(b"new".to_vec())),
            ("x64/data/ui/inner.rpf/settings/b.ymt".to_owned(), Op::Remove),
        ];
        let applied = install::apply(&game, None, "update_9.rpf", &ops, "inner").unwrap();
        assert!(applied.created);
        assert_eq!(applied.path, game.join("lml/replace/update_9/x64/data/ui/inner.rpf"));
        assert_eq!(read(&game, &format!("{inner}/a.ymt")), b"changed a");
        assert_eq!(read(&game, &format!("{inner}/new.ymt")), b"new");
        assert!(rage_mods::resolve(&game, &format!("{inner}/b.ymt"), None).is_err());
        let scanned: Vec<String> = rage_mods::scan(&game).into_iter().map(|f| f.rel).collect();
        assert_eq!(scanned, ["update_9.rpf/x64/data/top.ymt", "update_9.rpf/x64/data/ui/inner.rpf"]);

        // Undo: the copy, then the loose file, go again; the game is untouched.
        while install::undo_last(&game).unwrap().is_some() {}
        assert!(!loose.exists());
        assert!(!applied.path.exists());
        assert_eq!(fs::read(game.join("update_9.rpf")).unwrap(), before);
        assert_eq!(read(&game, &format!("{inner}/b.ymt")), b"inner b");
        fs::remove_dir_all(&game).ok();
    }
}
