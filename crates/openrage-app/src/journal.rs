//! Change journal for the mods folder, so every change can be undone.
//!
//! Stored in `<mods>/.openrage/journal/` (OpenIV ignores it): one small text
//! file per change, plus a binary file with the bytes the change overwrote.
//! Changes are undone in reverse order.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rage_rpf::{Backup, vfs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A game archive was copied into the mods folder (undo: delete the copy).
    Created,
    /// An archive in the mods folder was edited in place (undo: restore the backup).
    Edited { old_len: u64, new_len: u64 },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u64,
    /// Unix time.
    pub time: u64,
    pub kind: Kind,
    /// Archive path relative to the mods folder.
    pub archive: String,
    pub description: String,
}

pub struct Journal {
    mods: PathBuf,
    dir: PathBuf,
}

impl Journal {
    pub fn new(mods: &Path) -> Self {
        Self { mods: mods.to_owned(), dir: mods.join(".openrage").join("journal") }
    }

    fn next_id(&self) -> u64 {
        self.entries().map(|e| e.first().map_or(1, |x| x.id + 1)).unwrap_or(1)
    }

    fn write(&self, kind: &Kind, archive: &str, description: &str, region: Option<&[u8]>) -> Result<u64> {
        fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let id = self.next_id();
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut text = format!("time={time}\narchive={archive}\ndescription={}\n", description.replace('\n', " "));
        match kind {
            Kind::Created => text.push_str("kind=created\n"),
            Kind::Edited { old_len, new_len } => {
                text.push_str(&format!("kind=edited\nold_len={old_len}\nnew_len={new_len}\n"))
            }
        }
        if let Some(region) = region {
            let bin = self.dir.join(format!("{id:06}.bin"));
            fs::write(&bin, region).with_context(|| format!("writing {}", bin.display()))?;
        }
        // The text file is written last: an entry only counts once it is complete.
        let txt = self.dir.join(format!("{id:06}.txt"));
        fs::write(&txt, text).with_context(|| format!("writing {}", txt.display()))?;
        Ok(id)
    }

    pub fn record_created(&self, archive: &str, description: &str) -> Result<u64> {
        self.write(&Kind::Created, archive, description, None)
    }

    pub fn record_edit(&self, archive: &str, description: &str, backup: &Backup) -> Result<u64> {
        let kind = Kind::Edited { old_len: backup.old_len, new_len: backup.new_len };
        self.write(&kind, archive, description, Some(&backup.region))
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
                _ => continue,
            };
            out.push(Entry {
                id,
                time: get("time").parse().unwrap_or(0),
                kind,
                archive: get("archive"),
                description: get("description"),
            });
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.id));
        Ok(out)
    }

    /// Undoes the newest change. Returns what was undone.
    pub fn undo_last(&self) -> Result<Option<Entry>> {
        let Some(entry) = self.entries()?.into_iter().next() else { return Ok(None) };
        let path = vfs::find_case_insensitive(&self.mods, &entry.archive)
            .with_context(|| format!("{} no longer exists in the mods folder", entry.archive))?;
        let txt = self.dir.join(format!("{:06}.txt", entry.id));
        let bin = self.dir.join(format!("{:06}.bin", entry.id));
        match entry.kind {
            Kind::Created => {
                fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            }
            Kind::Edited { old_len, new_len } => {
                let region = fs::read(&bin).with_context(|| format!("reading {}", bin.display()))?;
                let backup = Backup { old_len, new_len, region };
                if let Err(e) = backup.restore(&path) {
                    bail!("cannot undo \"{}\": {e}", entry.description);
                }
            }
        }
        let _ = fs::remove_file(&bin);
        fs::remove_file(&txt)?;
        Ok(Some(entry))
    }
}
