//! Change journal for the mods folder, so every change can be undone.
//!
//! Stored in `<mods>/.openrage/journal/` (OpenIV ignores it): one small text
//! file per change, plus a binary file with the bytes the change overwrote.
//! Changes are undone in reverse order; all changes of one package install
//! share a group and can be undone together (uninstall).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rage_rpf::{Backup, vfs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A file was created in the mods folder (copy of a game archive, or a new
    /// archive); undo deletes it.
    Created,
    /// An archive in the mods folder was edited in place (undo: restore the backup).
    Edited { old_len: u64, new_len: u64 },
    /// A loose file was written (in the game folder if `in_game`, else in mods);
    /// undo restores the previous content or deletes the file if it did not exist.
    FileWritten { in_game: bool, existed: bool },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u64,
    /// Unix time.
    pub time: u64,
    pub kind: Kind,
    /// Path relative to the mods folder (or the game folder for files written there).
    pub archive: String,
    pub description: String,
    /// Package (or other batch) this change belongs to.
    pub group: Option<String>,
}

pub struct Journal {
    mods: PathBuf,
    dir: PathBuf,
    group: Option<String>,
}

impl Journal {
    pub fn new(mods: &Path) -> Self {
        Self { mods: mods.to_owned(), dir: mods.join(".openrage").join("journal"), group: None }
    }

    /// Records following changes as part of `group`.
    pub fn grouped(mut self, group: Option<&str>) -> Self {
        self.group = group.map(str::to_owned);
        self
    }

    fn game_dir(&self) -> PathBuf {
        self.mods.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    fn next_id(&self) -> u64 {
        self.entries().map(|e| e.first().map_or(1, |x| x.id + 1)).unwrap_or(1)
    }

    fn write(&self, kind: &Kind, path: &str, description: &str, data: Option<&[u8]>) -> Result<u64> {
        fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let id = self.next_id();
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut text = format!("time={time}\narchive={path}\ndescription={}\n", description.replace('\n', " "));
        if let Some(g) = &self.group {
            text.push_str(&format!("group={}\n", g.replace('\n', " ")));
        }
        match kind {
            Kind::Created => text.push_str("kind=created\n"),
            Kind::Edited { old_len, new_len } => {
                text.push_str(&format!("kind=edited\nold_len={old_len}\nnew_len={new_len}\n"))
            }
            Kind::FileWritten { in_game, existed } => {
                text.push_str(&format!("kind=file\nin_game={in_game}\nexisted={existed}\n"))
            }
        }
        if let Some(data) = data {
            let bin = self.dir.join(format!("{id:06}.bin"));
            fs::write(&bin, data).with_context(|| format!("writing {}", bin.display()))?;
        }
        // The text file is written last: an entry only counts once it is complete.
        let txt = self.dir.join(format!("{id:06}.txt"));
        fs::write(&txt, text).with_context(|| format!("writing {}", txt.display()))?;
        Ok(id)
    }

    pub fn record_created(&self, path: &str, description: &str) -> Result<u64> {
        self.write(&Kind::Created, path, description, None)
    }

    pub fn record_edit(&self, archive: &str, description: &str, backup: &Backup) -> Result<u64> {
        let kind = Kind::Edited { old_len: backup.old_len, new_len: backup.new_len };
        self.write(&kind, archive, description, Some(&backup.region))
    }

    /// Writes a loose file (relative to the game folder if `in_game`, else to
    /// the mods folder), recording its previous content first.
    pub fn write_file(&self, in_game: bool, rel: &str, data: &[u8], description: &str) -> Result<PathBuf> {
        let root = if in_game { self.game_dir() } else { self.mods.clone() };
        let path = vfs::find_case_insensitive(&root, rel).unwrap_or_else(|| root.join(rel));
        let previous = path.is_file().then(|| fs::read(&path)).transpose()?;
        self.write(&Kind::FileWritten { in_game, existed: previous.is_some() }, rel, description, previous.as_deref())?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, data).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Deletes a loose file in the mods folder, keeping its content for undo.
    pub fn delete_file(&self, rel: &str, description: &str) -> Result<()> {
        let path = vfs::find_case_insensitive(&self.mods, rel).filter(|p| p.is_file()).context("no such file")?;
        let previous = fs::read(&path)?;
        // Recorded as "written, existed": undo restores the content.
        self.write(&Kind::FileWritten { in_game: false, existed: true }, rel, description, Some(&previous))?;
        fs::remove_file(&path)?;
        Ok(())
    }

    /// All entries, newest first.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let Ok(rd) = fs::read_dir(&self.dir) else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for f in rd.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".txt").and_then(|n| n.parse::<u64>().ok()) else { continue };
            let text = fs::read_to_string(f.path())?;
            let get =
                |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or_default().to_owned();
            let kind = match get("kind").as_str() {
                "created" => Kind::Created,
                "edited" => Kind::Edited {
                    old_len: get("old_len").parse().unwrap_or(0),
                    new_len: get("new_len").parse().unwrap_or(0),
                },
                "file" => Kind::FileWritten { in_game: get("in_game") == "true", existed: get("existed") == "true" },
                _ => continue,
            };
            let group = Some(get("group")).filter(|g| !g.is_empty());
            out.push(Entry {
                id,
                time: get("time").parse().unwrap_or(0),
                kind,
                archive: get("archive"),
                description: get("description"),
                group,
            });
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.id));
        Ok(out)
    }

    fn undo(&self, entry: &Entry) -> Result<()> {
        let txt = self.dir.join(format!("{:06}.txt", entry.id));
        let bin = self.dir.join(format!("{:06}.bin", entry.id));
        match entry.kind {
            Kind::Created => {
                let path = vfs::find_case_insensitive(&self.mods, &entry.archive)
                    .with_context(|| format!("{} no longer exists in the mods folder", entry.archive))?;
                fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            }
            Kind::Edited { old_len, new_len } => {
                let path = vfs::find_case_insensitive(&self.mods, &entry.archive)
                    .with_context(|| format!("{} no longer exists in the mods folder", entry.archive))?;
                let region = fs::read(&bin).with_context(|| format!("reading {}", bin.display()))?;
                let backup = Backup { old_len, new_len, region };
                if let Err(e) = backup.restore(&path) {
                    bail!("cannot undo \"{}\": {e}", entry.description);
                }
            }
            Kind::FileWritten { in_game, existed } => {
                let root = if in_game { self.game_dir() } else { self.mods.clone() };
                let path =
                    vfs::find_case_insensitive(&root, &entry.archive).unwrap_or_else(|| root.join(&entry.archive));
                if existed {
                    fs::write(&path, fs::read(&bin)?).with_context(|| format!("restoring {}", path.display()))?;
                } else if path.exists() {
                    fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
                }
            }
        }
        let _ = fs::remove_file(&bin);
        fs::remove_file(&txt)?;
        Ok(())
    }

    /// Undoes the newest change. Returns what was undone.
    pub fn undo_last(&self) -> Result<Option<Entry>> {
        let Some(entry) = self.entries()?.into_iter().next() else { return Ok(None) };
        self.undo(&entry)?;
        Ok(Some(entry))
    }

    /// Undoes all changes of a group (newest first). Refuses if a later change
    /// outside the group touched the same file.
    pub fn undo_group(&self, group: &str) -> Result<Vec<Entry>> {
        let entries = self.entries()?;
        let members: Vec<&Entry> = entries.iter().filter(|e| e.group.as_deref() == Some(group)).collect();
        if members.is_empty() {
            bail!("nothing recorded for \"{group}\"");
        }
        for m in &members {
            if let Some(later) = entries.iter().find(|e| {
                e.id > m.id && e.group.as_deref() != Some(group) && e.archive.eq_ignore_ascii_case(&m.archive)
            }) {
                bail!("\"{}\" changed {} later; undo that first", later.description, later.archive);
            }
        }
        let mut done = Vec::new();
        for m in members {
            self.undo(m)?;
            done.push(m.clone());
        }
        Ok(done)
    }

    /// Groups (packages) in the journal, newest first, with their entry counts.
    pub fn groups(&self) -> Result<Vec<(String, usize)>> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for e in self.entries()? {
            if let Some(g) = e.group {
                match out.iter_mut().find(|(name, _)| *name == g) {
                    Some((_, n)) => *n += 1,
                    None => out.push((g, 1)),
                }
            }
        }
        Ok(out)
    }
}
