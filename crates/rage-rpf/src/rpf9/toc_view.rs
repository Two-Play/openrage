//! Adapter from verified TOC entries to the application-facing archive view.
use super::recovery::{Entry, Kind};
use super::{Archive, NameIndex, Rpf9Error};
use crate::{
    RpfError,
    rpf9_exe::{ExtractedMaterial, extract_scarlett_1_0_2_1},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub(super) struct TocView {
    pub archive: Mutex<Archive>,
    pub keys: Arc<ExtractedMaterial>,
    pub entries: Arc<Vec<Entry>>,
    pub names: HashMap<u32, String>,
    pub dictionary: Option<PathBuf>,
}
impl std::fmt::Debug for TocView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TocView").field("entries", &self.entries.len()).finish_non_exhaustive()
    }
}

pub(super) fn error(e: Rpf9Error) -> RpfError {
    RpfError::Io(std::io::Error::other(e.to_string()))
}

pub(super) fn discover(path: &Path) -> Result<Option<TocView>, RpfError> {
    static KEYS: OnceLock<Mutex<HashMap<PathBuf, Arc<ExtractedMaterial>>>> = OnceLock::new();
    for dir in path.ancestors().skip(1).take(8) {
        let exe = dir.join("game_scarlett_master.exe");
        if !exe.is_file() {
            continue;
        }
        let mut cache = KEYS.get_or_init(Default::default).lock().map_err(|_| RpfError::Cancelled)?;
        let keys = if let Some(keys) = cache.get(&exe) {
            Arc::clone(keys)
        } else {
            let keys = Arc::new(
                extract_scarlett_1_0_2_1(&exe).map_err(|e| RpfError::Io(std::io::Error::other(e.to_string())))?,
            );
            cache.insert(exe, Arc::clone(&keys));
            keys
        };
        drop(cache);
        let dictionary = std::env::var_os("RPF9_NAMES").map(PathBuf::from).or_else(|| {
            let p = dir.join("rpf9-names.txt");
            p.is_file().then_some(p)
        });
        return Ok(Some(TocView::new(Archive::open(path, &keys).map_err(error)?, keys, dictionary)?));
    }
    Ok(None)
}

impl TocView {
    pub fn new(archive: Archive, keys: Arc<ExtractedMaterial>, dictionary: Option<PathBuf>) -> Result<Self, RpfError> {
        type CachedNames = (PathBuf, std::time::SystemTime, u64, Arc<NameIndex>);
        static NAMES: OnceLock<Mutex<Option<CachedNames>>> = OnceLock::new();
        let index = if let Some(path) = &dictionary {
            let meta = std::fs::metadata(path)?;
            let modified = meta.modified()?;
            let mut slot = NAMES.get_or_init(Default::default).lock().map_err(|_| RpfError::Cancelled)?;
            if !slot.as_ref().is_some_and(|(p, m, len, _)| p == path && *m == modified && *len == meta.len()) {
                *slot = Some((path.clone(), modified, meta.len(), Arc::new(NameIndex::load_all_candidates(path)?)));
            }
            slot.as_ref().map(|(_, _, _, names)| Arc::clone(names))
        } else {
            None
        };
        let mut entries = Vec::with_capacity(archive.metadata.entries.len());
        let mut names = HashMap::new();
        let mut used_names = std::collections::HashSet::new();
        for e in &archive.metadata.entries {
            let extent =
                e.extent.as_ref().ok_or_else(|| RpfError::Io(std::io::Error::other("unsupported TOC extent")))?;
            let resource = e.resource_flags.is_some();
            let entry = Entry {
                offset: extent.offset,
                stored: extent.stored_size,
                size: extent.unpacked_size + if resource { 16 } else { 0 },
                streams: e.blocks.as_ref().map_or(1, |b| b.len() as u32),
                kind: Kind::Toc { index: e.index, resource, file_type: extent.flags as u8 },
                head: [0; 16],
                head_len: 0,
            };
            let ext = entry.extension();
            let hash = e.name_hash.unwrap_or(e.hash);
            let candidates: Vec<_> = index.as_ref().map(|i| i.candidates(hash).collect()).unwrap_or_default();
            let candidate = (candidates.len() == 1).then(|| candidates[0]).filter(|p| {
                !p.starts_with('/') && !p.contains(':') && !p.split('/').any(|s| s == ".." || s.is_empty())
            });
            let name = match candidate {
                Some(p) if extent.flags as u8 != 0xff => format!("{p}.{ext}"),
                Some(p) => p.to_owned(),
                None => format!("{ext}/{hash:016x}.{ext}"),
            };
            let name =
                if used_names.insert(name.clone()) { name } else { format!("{ext}/{:06}_{hash:016x}.{ext}", e.index) };
            names.insert(e.index, name);
            entries.push(entry);
        }
        entries.sort_by_key(|e| e.offset);
        Ok(Self { archive: Mutex::new(archive), keys, entries: Arc::new(entries), names, dictionary })
    }
}
