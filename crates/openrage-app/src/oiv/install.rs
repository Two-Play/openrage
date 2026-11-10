//! Previewing and installing packages into the mods folder.
//!
//! The same walk over the package script runs in two modes: a dry run that
//! reads everything and reports what would happen (and what cannot work), and
//! the real installation. Archive changes are batched per top-level archive
//! and written with the in-place editor; text and XML edits read the current
//! file (including earlier changes of the same package). All journal entries
//! carry the package's group, so the package can be uninstalled as a whole.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use rage_crypto::GtaVKeys;
use rage_rpf::ArchiveWriter;
use rage_rpf::vfs::{self, Node};

use super::package::{Command, Package};
use super::{text, xml};
use crate::install::{self, Op};
use crate::journal::{Entry, Journal};

/// One planned (or performed) change.
#[derive(Debug, Clone)]
pub struct Step {
    /// "Add", "Replace", "Delete", "Edit", "Create".
    pub action: &'static str,
    /// Virtual path as the game sees it.
    pub target: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub steps: Vec<Step>,
    /// Things that will be skipped or might not do what the author intended.
    pub warnings: Vec<String>,
    /// Problems that prevent installation.
    pub blockers: Vec<String>,
}

/// The journal group used for a package.
pub fn group_name(pkg: &Package) -> String {
    format!("{} {}", pkg.metadata.name, pkg.metadata.version).trim().to_owned()
}

fn norm(p: &str) -> String {
    p.replace('\\', "/").split('/').filter(|s| !s.is_empty() && *s != ".").collect::<Vec<_>>().join("/")
}

fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a}/{b}"),
    }
}

#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
}

fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8Bom);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return (String::from_utf16_lossy(&units), Encoding::Utf16Le);
    }
    (String::from_utf8_lossy(bytes).into_owned(), Encoding::Utf8)
}

fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        Encoding::Utf16Le => [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect(),
    }
}

struct Scope {
    /// Top-level archive (relative path), or None for loose files.
    archive: Option<String>,
    /// Path of the current nested archive inside the top-level archive.
    prefix: String,
}

struct Run<'a> {
    game_dir: &'a Path,
    keys: Option<Arc<GtaVKeys>>,
    pkg: &'a Package,
    dry: bool,
    group: String,
    report: Report,
    /// Archive changes not yet written, per top-level archive (in order).
    pending: Vec<(String, Vec<(String, Op)>)>,
    /// Contents written by this package so far (None = deleted), by lowercase virtual path.
    staged: HashMap<String, Option<Vec<u8>>>,
    /// Archives created by this package (dry run), by lowercase virtual path.
    created: HashSet<String>,
}

impl Run<'_> {
    fn vpath(scope: &Scope, p: &str) -> String {
        match &scope.archive {
            Some(a) => join(&join(a, &scope.prefix), &norm(p)),
            None => norm(p),
        }
    }

    fn resolve(&self, vpath: &str) -> Option<Node> {
        rage_mods::resolve(self.game_dir, vpath, self.keys.clone()).ok().map(|(n, _)| n)
    }

    fn exists(&self, vpath: &str) -> bool {
        let key = vpath.to_ascii_lowercase();
        if let Some(s) = self.staged.get(&key) {
            return s.is_some();
        }
        self.created.contains(&key) || self.resolve(vpath).is_some()
    }

    fn read(&self, vpath: &str) -> Option<Vec<u8>> {
        if let Some(s) = self.staged.get(&vpath.to_ascii_lowercase()) {
            return s.clone();
        }
        match self.resolve(vpath)? {
            Node::ArchiveFile(rpf, id) => rpf.read(id).ok(),
            Node::PlainFile(p) => std::fs::read(p).ok(),
            _ => None,
        }
    }

    fn step(&mut self, action: &'static str, target: &str, detail: impl Into<String>) {
        self.report.steps.push(Step { action, target: target.to_owned(), detail: detail.into() });
    }

    fn queue(&mut self, archive: &str, inner: String, op: Op) {
        let vpath = join(archive, &inner).to_ascii_lowercase();
        match &op {
            Op::Set(data) => {
                self.staged.insert(vpath, Some(data.clone()));
            }
            Op::Remove => {
                self.staged.insert(vpath, None);
            }
            _ => {}
        }
        match self.pending.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(archive)) {
            Some((_, ops)) => ops.push((inner, op)),
            None => self.pending.push((archive.to_owned(), vec![(inner, op)])),
        }
    }

    fn flush(&mut self) -> Result<()> {
        for (archive, ops) in std::mem::take(&mut self.pending) {
            if self.dry || ops.is_empty() {
                continue;
            }
            let description = format!("{}: {} change(s) in {archive}", self.group, ops.len());
            install::apply_grouped(self.game_dir, self.keys.clone(), &archive, &ops, &description, Some(&self.group))?;
        }
        Ok(())
    }

    /// Writes a loose file: replacing a game file goes to the mods folder,
    /// a new file goes to the game folder (where loaders expect `.asi`, `.dll`, …).
    fn write_loose(&mut self, rel: &str, data: Vec<u8>, action: &'static str) -> Result<()> {
        let in_mods = rage_mods::mods_dir(self.game_dir)
            .and_then(|m| vfs::find_case_insensitive(&m, rel))
            .is_some_and(|p| p.is_file());
        let in_game = vfs::find_case_insensitive(self.game_dir, rel).is_some_and(|p| p.is_file());
        let to_game_folder = !in_mods && !in_game;
        let detail = if to_game_folder { "new file in the game folder" } else { "in the mods folder" };
        self.step(action, rel, detail);
        self.staged.insert(rel.to_ascii_lowercase(), Some(data.clone()));
        if !self.dry {
            let mods = install::mods_dir_or_create(self.game_dir)?;
            let description = format!("{}: wrote {rel}", self.group);
            Journal::new(&mods).grouped(Some(&self.group)).write_file(to_game_folder, rel, &data, &description)?;
        }
        Ok(())
    }

    fn write_edited(&mut self, scope: &Scope, path: &str, data: Vec<u8>) -> Result<()> {
        match &scope.archive {
            Some(a) => {
                let inner = join(&scope.prefix, &norm(path));
                self.queue(&a.clone(), inner, Op::Set(data));
                Ok(())
            }
            None => self.write_loose(&norm(path), data, "Edit"),
        }
    }

    fn walk(&mut self, commands: &[Command], scope: &Scope) -> Result<()> {
        for cmd in commands {
            match cmd {
                Command::Add { source, target } => {
                    let Some(data) = self.pkg.file(source).map(<[u8]>::to_vec) else {
                        self.report.blockers.push(format!("the package lacks content file \"{source}\""));
                        continue;
                    };
                    let vpath = Self::vpath(scope, target);
                    let action = if self.exists(&vpath) { "Replace" } else { "Add" };
                    match &scope.archive {
                        Some(a) => {
                            self.step(action, &vpath, format!("{} bytes", data.len()));
                            self.queue(&a.clone(), join(&scope.prefix, &norm(target)), Op::Set(data));
                        }
                        None => self.write_loose(&norm(target), data, action)?,
                    }
                }
                Command::Delete { target } => {
                    let vpath = Self::vpath(scope, target);
                    if !self.exists(&vpath) {
                        self.report.warnings.push(format!("{vpath} does not exist; nothing to delete"));
                        continue;
                    }
                    match &scope.archive {
                        Some(a) => {
                            self.step("Delete", &vpath, "");
                            self.queue(&a.clone(), join(&scope.prefix, &norm(target)), Op::Remove);
                        }
                        None => {
                            let rel = norm(target);
                            let in_mods = rage_mods::mods_dir(self.game_dir)
                                .and_then(|m| vfs::find_case_insensitive(&m, &rel))
                                .is_some_and(|p| p.is_file());
                            if !in_mods {
                                self.report.warnings.push(format!("{rel}: game files are never deleted; skipped"));
                                continue;
                            }
                            self.step("Delete", &rel, "from the mods folder");
                            self.staged.insert(rel.to_ascii_lowercase(), None);
                            if !self.dry {
                                let mods = install::mods_dir_or_create(self.game_dir)?;
                                Journal::new(&mods)
                                    .grouped(Some(&self.group))
                                    .delete_file(&rel, &format!("{}: deleted {rel}", self.group))?;
                            }
                        }
                    }
                }
                Command::Archive { path, create, kind, commands } => {
                    if !kind.eq_ignore_ascii_case("RPF7") {
                        self.report.blockers.push(format!("archive type {kind} is not supported for GTA V"));
                        continue;
                    }
                    let vpath = Self::vpath(scope, path);
                    let exists = self.exists(&vpath);
                    if !exists && !create {
                        self.report.blockers.push(format!("archive {vpath} does not exist"));
                        continue;
                    }
                    let inner_scope = match &scope.archive {
                        None => {
                            let rel = norm(path);
                            if !exists {
                                self.step("Create", &rel, "new archive in the mods folder");
                                self.created.insert(rel.to_ascii_lowercase());
                                if !self.dry {
                                    self.flush()?;
                                    install::create_archive(
                                        self.game_dir,
                                        &rel,
                                        &format!("{}: created {rel}", self.group),
                                        Some(&self.group),
                                    )?;
                                }
                            }
                            Scope { archive: Some(rel), prefix: String::new() }
                        }
                        Some(a) => {
                            let inner = join(&scope.prefix, &norm(path));
                            if !exists {
                                self.step("Create", &vpath, "new nested archive");
                                self.created.insert(vpath.to_ascii_lowercase());
                                self.queue(&a.clone(), inner.clone(), Op::Set(ArchiveWriter::new().build()?));
                                // Written now so that changes inside it can find it.
                                self.flush()?;
                            }
                            Scope { archive: Some(a.clone()), prefix: inner }
                        }
                    };
                    self.walk(commands, &inner_scope)?;
                }
                Command::Text { path, create, commands } => {
                    let vpath = Self::vpath(scope, path);
                    let (before, enc) = match self.read(&vpath) {
                        Some(bytes) => decode(&bytes),
                        None if *create => (String::new(), Encoding::Utf8),
                        None => {
                            self.report.blockers.push(format!("text file {vpath} does not exist"));
                            continue;
                        }
                    };
                    let (after, notes) = text::apply(&before, commands);
                    self.report.warnings.extend(notes.into_iter().map(|n| format!("{vpath}: {n}")));
                    self.step("Edit", &vpath, format!("{} text command(s)", commands.len()));
                    self.write_edited(scope, path, encode(&after, enc))?;
                }
                Command::Xml { path, commands } => {
                    let vpath = Self::vpath(scope, path);
                    let Some(bytes) = self.read(&vpath) else {
                        self.report.blockers.push(format!("XML file {vpath} does not exist"));
                        continue;
                    };
                    let (mut doc, enc) = decode(&bytes);
                    let mut failed = false;
                    for c in commands {
                        match xml::apply(&doc, c) {
                            Ok(d) => doc = d,
                            Err(e) => {
                                self.report.blockers.push(format!("{vpath}: {e:#}"));
                                failed = true;
                                break;
                            }
                        }
                    }
                    if failed {
                        continue;
                    }
                    self.step("Edit", &vpath, format!("{} XML command(s)", commands.len()));
                    self.write_edited(scope, path, encode(&doc, enc))?;
                }
                Command::Pso { path } => {
                    self.report
                        .blockers
                        .push(format!("{}: PSO/META edits are not supported yet", Self::vpath(scope, path)));
                }
                Command::Defragment { archive } => {
                    self.report.warnings.push(format!("defragmentation of {archive} skipped (only saves disk space)"));
                }
            }
        }
        Ok(())
    }
}

fn run(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, pkg: &Package, dry: bool) -> Result<Report> {
    let mut r = Run {
        game_dir,
        keys,
        pkg,
        dry,
        group: group_name(pkg),
        report: Report::default(),
        pending: Vec::new(),
        staged: HashMap::new(),
        created: HashSet::new(),
    };
    r.walk(&pkg.commands, &Scope { archive: None, prefix: String::new() })?;
    r.flush()?;
    Ok(r.report)
}

/// What installing the package would do, without changing anything.
pub fn preview(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, pkg: &Package) -> Result<Report> {
    run(game_dir, keys, pkg, true)
}

/// Installs the package into the mods folder. Refuses if the preview found blockers.
pub fn install(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, pkg: &Package) -> Result<Report> {
    let plan = preview(game_dir, keys.clone(), pkg)?;
    if !plan.blockers.is_empty() {
        bail!("the package cannot be installed:\n  {}", plan.blockers.join("\n  "));
    }
    run(game_dir, keys, pkg, false)
}

/// Undoes everything a package installed.
pub fn uninstall(game_dir: &Path, group: &str) -> Result<Vec<Entry>> {
    let Some(mods) = rage_mods::mods_dir(game_dir) else { bail!("no mods folder") };
    Journal::new(&mods).undo_group(group)
}
