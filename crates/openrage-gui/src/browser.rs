//! UI-independent browsing logic: folder listings, the folder tree and history.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_crypto::GtaVKeys;
use rage_mods::Origin;
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, EntryKind, Rpf7, RpfError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Dir,
    Archive,
    File,
    Resource,
    /// A texture inside an opened texture dictionary.
    Texture,
    /// A sound inside an opened audio container.
    Sound,
}

impl ItemKind {
    pub fn is_container(self) -> bool {
        matches!(self, Self::Dir | Self::Archive)
    }
}

/// How an item relates to the mods folder (overlay view only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// A game file replaced by a file in `mods`.
    Replaced,
    /// Exists only in `mods`.
    New,
    /// A game folder with modded files somewhere below it.
    ContainsMods,
    /// Inside a modded archive: not in the original.
    Added,
    /// Inside a modded archive: differs from the original.
    Changed,
    /// In the original, but removed by the mod (shown greyed out).
    Removed,
}

impl Mark {
    pub fn label(self) -> &'static str {
        match self {
            Self::Replaced => "mod",
            Self::New => "new",
            Self::ContainsMods => "has mods",
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Removed => "removed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub size: u64,
    pub info: String,
    /// Entry inside the current archive (None for items on disk and search hits).
    pub entry: Option<EntryId>,
    /// Full virtual path, set for search hits (which come from many places).
    pub vpath: Option<String>,
    pub mark: Option<Mark>,
    /// Shown in the location column instead of the path (history entries).
    pub detail: Option<String>,
}

/// Where a listing comes from.
#[derive(Clone)]
pub enum Location {
    Disk(PathBuf),
    Archive(Arc<Rpf7>, EntryId),
    /// A folder inside a Red Dead Redemption 2 archive (read-only).
    Rpf8(Arc<rage_rpf::Rpf8>, String),
    /// Results of a search; `summary` describes hash lookups etc.
    Search {
        query: String,
        summary: String,
    },
    /// All changes made by the mods folder.
    Changes {
        summary: String,
    },
    /// The change journal of the mods folder.
    History,
    /// An opened texture dictionary (`.ytd`); items are its textures in order.
    Textures {
        dict: Arc<rage_formats::ytd::TextureDictionary>,
        /// The textures of a particle dictionary (`.ypt`): shown and exported, not changed.
        read_only: bool,
    },
    /// An opened audio container (`.awc`); items are its sounds (or its one
    /// multi-channel stream).
    Audio {
        awc: Arc<rage_formats::awc::Awc>,
    },
}

/// Whether a file name is an audio container that can be opened like a folder.
pub fn is_audio(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".awc")
}

/// Whether a file name is a texture dictionary that can be opened like a folder.
pub fn is_texture_dict(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".ytd") || lower.ends_with(".ypt")
}

/// Particle dictionaries open like texture dictionaries, read-only.
pub fn is_particles(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".ypt")
}

/// The virtual path of the mod change list.
pub const CHANGES_PATH: &str = "!mods";
/// The virtual path of the change journal.
pub const HISTORY_PATH: &str = "!history";

/// Virtual paths starting with `?` are searches.
pub fn search_query(vpath: &str) -> Option<&str> {
    vpath.strip_prefix('?').map(str::trim).filter(|q| !q.is_empty())
}

pub struct Listing {
    /// Normalized virtual path (`/`-separated, no leading or trailing slash).
    pub vpath: String,
    /// Where the listed folder comes from in the overlay view.
    pub origin: Origin,
    pub location: Location,
    pub items: Vec<Item>,
}

pub fn normalize(vpath: &str) -> String {
    if vpath.starts_with('?') || vpath.starts_with('!') {
        return vpath.trim().to_owned();
    }
    vpath.replace('\\', "/").split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

pub fn parent(vpath: &str) -> Option<String> {
    if vpath.is_empty() || vpath.starts_with('?') || vpath.starts_with('!') {
        return None;
    }
    Some(vpath.rsplit_once('/').map(|(p, _)| p.to_owned()).unwrap_or_default())
}

pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") }
}

/// Shortens a path from the front, keeping whole trailing segments:
/// `update/x64/dlcpacks/mpbeach/dlc.rpf/common/data` → `…/mpbeach/dlc.rpf/common/data`.
pub fn shorten_front(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        return path.to_owned();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut len = 2; // "…/"
    for seg in path.rsplit('/') {
        let add = seg.chars().count() + usize::from(!kept.is_empty());
        if len + add > max_chars && !kept.is_empty() {
            break;
        }
        len += add;
        kept.push(seg);
    }
    kept.reverse();
    format!("…/{}", kept.join("/"))
}

/// Compact location for search results: the DLC pack (if any) followed by
/// the tail of the path, e.g. `mpheist4: …/data/lang/germandlc.rpf`.
pub fn short_location(path: &str, max_chars: usize) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let dlc = segs.windows(2).rev().find(|w| matches!(w[0], "dlcpacks" | "dlc_patch")).map(|w| w[1]);
    match dlc {
        Some(name) => {
            let rest = path.split_once(&format!("/{name}/")).map_or("", |(_, r)| r);
            let rest = rest.strip_prefix("dlc.rpf/").or_else(|| rest.strip_prefix("dlc1.rpf/")).unwrap_or(rest);
            if rest.is_empty() {
                return format!("{name} (DLC pack)");
            }
            format!("{name}: {}", shorten_front(rest, max_chars.saturating_sub(name.len() + 2).max(12)))
        }
        None => shorten_front(path, max_chars),
    }
}

fn last_segment(vpath: &str) -> String {
    vpath.rsplit('/').next().unwrap_or_default().to_owned()
}

/// Lists a virtual path. If the path names a file, its folder is listed and
/// the file name is returned so the caller can select it.
///
/// With `overlay`, the game is shown as it sees itself with OpenIV.asi: files
/// in `mods` replace game files, and game folders merge with their `mods` counterpart.
pub fn list(
    game_dir: &Path,
    vpath: &str,
    keys: Option<Arc<GtaVKeys>>,
    overlay: bool,
) -> Result<(Listing, Option<String>), RpfError> {
    let vpath = normalize(vpath);
    let resolve = |p: &str| -> Result<(Node, Origin), RpfError> {
        if overlay {
            rage_mods::resolve(game_dir, p, keys.clone())
        } else {
            Ok((vfs::resolve(game_dir, p, keys.clone())?, Origin::Base))
        }
    };
    let (node, origin) = match resolve(&vpath) {
        Ok(found) => found,
        // `…/name.ytd/texture` or `…/name.awc/sound`: open the container and select the item.
        Err(RpfError::NotFound(_)) if parent(&vpath).is_some_and(|p| is_texture_dict(&p) || is_audio(&p)) => {
            let dict_path = parent(&vpath).unwrap_or_default();
            let (listing, _) = list(game_dir, &dict_path, keys, overlay)?;
            return Ok((listing, Some(last_segment(&vpath))));
        }
        Err(e) => return Err(e),
    };
    match node {
        Node::ArchiveFile(rpf, id)
            if is_texture_dict(&rpf.entry(id).name) && rpf.entry(id).resource_version().is_some() =>
        {
            Ok((list_textures(vpath, rpf, id, origin)?, None))
        }
        Node::ArchiveFile(rpf, id) if is_audio(&rpf.entry(id).name) => {
            Ok((list_audio(vpath, rpf, id, origin, keys.as_deref())?, None))
        }
        Node::Dir(dir) => {
            let mut listing = list_disk(vpath, dir, origin)?;
            if overlay {
                merge_mods(game_dir, &mut listing);
            }
            Ok((listing, None))
        }
        Node::ArchiveDir(rpf, dir) => Ok((list_archive(vpath, rpf, dir, origin), None)),
        Node::Rpf8Dir(rpf, dir) => Ok((list_rpf8(vpath, rpf, dir, origin), None)),
        Node::Rpf8File(rpf, i) if is_texture_dict(&rpf.entry_name(i)) => {
            Ok((list_rpf8_textures(vpath, rpf, i, origin)?, None))
        }
        Node::ArchiveFile(..) | Node::PlainFile(_) | Node::Rpf8File(..) => {
            let name = last_segment(&vpath);
            let (listing, _) = list(game_dir, &parent(&vpath).unwrap_or_default(), keys, overlay)?;
            Ok((listing, Some(name)))
        }
    }
}

/// Adds the `mods` counterpart of a disk folder to its listing and marks items.
fn merge_mods(game_dir: &Path, listing: &mut Listing) {
    merge_mods_inner(game_dir, listing);
    // OpenRAGE's journal folder inside `mods` is not part of the game.
    listing.items.retain(|i| !i.name.eq_ignore_ascii_case(rage_mods::overlay::OPENRAGE_FOLDER));
}

fn merge_mods_inner(game_dir: &Path, listing: &mut Listing) {
    let Some(mods) = rage_mods::mods_dir(game_dir) else { return };
    if listing.vpath.is_empty() {
        listing.items.retain(|i| !i.name.eq_ignore_ascii_case(rage_mods::overlay::MODS_FOLDER));
    }

    if listing.origin == Origin::Mods {
        listing.items.iter_mut().for_each(|i| i.mark = Some(Mark::New));
        return;
    }
    let Some(mod_dir) = vfs::find_case_insensitive(&mods, &listing.vpath).filter(|p| p.is_dir()) else { return };
    let Ok(Listing { items: mod_items, .. }) = list_disk(listing.vpath.clone(), mod_dir, Origin::Mods) else { return };
    for m in mod_items {
        match listing.items.iter_mut().find(|i| i.name.eq_ignore_ascii_case(&m.name)) {
            Some(existing) if m.kind == ItemKind::Dir => existing.mark = Some(Mark::ContainsMods),
            Some(existing) => {
                existing.size = m.size;
                existing.mark = Some(Mark::Replaced);
            }
            None => listing.items.push(Item { mark: Some(Mark::New), ..m }),
        }
    }
}

fn list_textures(vpath: String, rpf: Arc<Rpf7>, id: EntryId, origin: Origin) -> Result<Listing, RpfError> {
    let res = rpf.read_resource(id)?;
    let bad = |reason: String| RpfError::BadResource { path: vpath.clone(), reason };
    let read_only = is_particles(&rpf.entry(id).name);
    let dict = if read_only {
        let ptfx = rage_formats::ypt::parse(&res).map_err(|e| bad(e.to_string()))?;
        ptfx.textures.unwrap_or_else(|| rage_formats::ytd::TextureDictionary {
            textures: Vec::new(),
            errors: Vec::new(),
            layout: rage_formats::ytd::Layout::Enhanced,
            version: res.version,
            root: Vec::new(),
            stored: Vec::new(),
        })
    } else {
        rage_formats::ytd::parse(&res).map_err(|e| bad(e.to_string()))?
    };
    let items = dict
        .textures
        .iter()
        .map(|t| Item {
            name: t.name.clone(),
            kind: ItemKind::Texture,
            size: t.data.len() as u64,
            info: format!("{}×{} {}", t.width, t.height, t.format.name()),
            entry: None,
            vpath: None,
            mark: None,
            detail: None,
        })
        .collect();
    Ok(Listing { vpath, origin, location: Location::Textures { dict: Arc::new(dict), read_only }, items })
}

/// The textures of a Red Dead Redemption 2 texture dictionary (read-only).
fn list_rpf8_textures(vpath: String, rpf: Arc<rage_rpf::Rpf8>, i: usize, origin: Origin) -> Result<Listing, RpfError> {
    let entry = rpf.entries().and_then(|e| e.get(i)).ok_or_else(|| RpfError::NotFound(vpath.clone()))?;
    let res = rpf.read_resource(entry)?;
    let dict = rage_formats::ytd::parse_rdr2(&res)
        .map_err(|e| RpfError::BadResource { path: vpath.clone(), reason: e.to_string() })?;
    let items = dict
        .textures
        .iter()
        .map(|t| Item {
            name: t.name.clone(),
            kind: ItemKind::Texture,
            size: t.data.len() as u64,
            info: format!("{}×{} {}", t.width, t.height, t.format.name()),
            entry: None,
            vpath: None,
            mark: None,
            detail: None,
        })
        .collect();
    Ok(Listing { vpath, origin, location: Location::Textures { dict: Arc::new(dict), read_only: true }, items })
}

fn list_audio(
    vpath: String,
    rpf: Arc<Rpf7>,
    id: EntryId,
    origin: Origin,
    keys: Option<&GtaVKeys>,
) -> Result<Listing, RpfError> {
    let data = rpf.read(id)?;
    let awc = openrage_app::audio::parse(data, keys)
        .map_err(|e| RpfError::BadResource { path: vpath.clone(), reason: format!("{e:#}") })?;
    let item = |name: String, size: u64, info: String| Item {
        name,
        kind: ItemKind::Sound,
        size,
        info,
        entry: None,
        vpath: None,
        mark: None,
        detail: None,
    };
    let items = match &awc.stream_format {
        Some(f) => {
            let secs = awc.sounds.first().map_or(0.0, |s| s.duration_secs());
            let size = awc.chunk(0, rage_formats::awc::CHUNK_DATA).map_or(0, |c| c.size as u64);
            vec![item(
                format!("{} (stream)", last_segment(&vpath).trim_end_matches(".awc")),
                size,
                format!("{} · {} ch", seconds(secs), f.channels),
            )]
        }
        None => awc
            .sounds
            .iter()
            .map(|s| {
                let stream = awc.streams.iter().position(|(sid, _)| *sid == s.id);
                let size =
                    stream.and_then(|i| awc.chunk(i, rage_formats::awc::CHUNK_DATA)).map_or(0, |c| c.size as u64);
                item(format!("{:08x}", s.id), size, seconds(s.duration_secs()))
            })
            .collect(),
    };
    Ok(Listing { vpath, origin, location: Location::Audio { awc: Arc::new(awc) }, items })
}

/// `83.4` → `1:23.4`.
pub fn seconds(secs: f32) -> String {
    let m = (secs / 60.0).floor() as u32;
    format!("{m}:{:04.1}", secs - m as f32 * 60.0)
}

fn list_disk(vpath: String, dir: PathBuf, origin: Origin) -> Result<Listing, RpfError> {
    let mut items = Vec::new();
    for entry in fs::read_dir(&dir)?.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = if meta.is_dir() {
            ItemKind::Dir
        } else if name.to_ascii_lowercase().ends_with(".rpf") {
            ItemKind::Archive
        } else {
            ItemKind::File
        };
        let size = if meta.is_dir() { 0 } else { meta.len() };
        items.push(Item { name, kind, size, info: String::new(), entry: None, vpath: None, mark: None, detail: None });
    }
    Ok(Listing { vpath, origin, location: Location::Disk(dir), items })
}

/// A folder of an RDR2 archive: folders from the entries' paths, nested
/// archives, files (unnamed ones under `hash/`).
fn list_rpf8(vpath: String, rpf: Arc<rage_rpf::Rpf8>, dir: String, origin: Origin) -> Listing {
    use rage_rpf::rpf8::Compression;
    let (dirs, files) = rpf.list_dir(&dir);
    let mut items: Vec<Item> = dirs
        .into_iter()
        .map(|name| Item {
            name,
            kind: ItemKind::Dir,
            size: 0,
            info: String::new(),
            entry: None,
            vpath: None,
            mark: None,
            detail: None,
        })
        .collect();
    let entries = rpf.entries().unwrap_or_default();
    for i in files {
        let e = &entries[i];
        let full = rpf.entry_name(i);
        let name = full.rsplit('/').next().unwrap_or(&full).to_owned();
        let (kind, info) = if e.ext_id == 0 {
            (ItemKind::Archive, String::new())
        } else {
            let mut info = Vec::new();
            if let Some(v) = e.resource_version() {
                info.push(format!("v{v}"));
            }
            match e.compression {
                Compression::Oodle => info.push("oodle".into()),
                Compression::Deflate => info.push("deflate".into()),
                _ => {}
            }
            if e.is_encrypted() {
                info.push("enc".into());
            }
            (if e.is_resource { ItemKind::Resource } else { ItemKind::File }, info.join(", "))
        };
        items.push(Item { name, kind, size: e.size(), info, entry: None, vpath: None, mark: None, detail: None });
    }
    Listing { vpath, origin, location: Location::Rpf8(rpf, dir), items }
}

fn list_archive(vpath: String, rpf: Arc<Rpf7>, dir: EntryId, origin: Origin) -> Listing {
    let items = rpf
        .children(dir)
        .map(|id| {
            let e = rpf.entry(id);
            let (kind, info) = match e.kind {
                EntryKind::Directory { .. } => (ItemKind::Dir, String::new()),
                EntryKind::Binary { .. } if e.is_archive() => (ItemKind::Archive, String::new()),
                EntryKind::Binary { stored_size, encrypted, .. } => {
                    let mut info = Vec::new();
                    if stored_size > 0 {
                        info.push("deflate");
                    }
                    if encrypted {
                        info.push("enc");
                    }
                    (ItemKind::File, info.join(", "))
                }
                EntryKind::Resource { .. } => {
                    (ItemKind::Resource, e.resource_version().map(|v| format!("v{v}")).unwrap_or_default())
                }
            };
            Item {
                name: e.name.clone(),
                kind,
                size: e.size(),
                info,
                entry: Some(id),
                vpath: None,
                mark: None,
                detail: None,
            }
        })
        .collect();
    Listing { vpath, origin, location: Location::Archive(rpf, dir), items }
}

/// Sort columns: 0 name, 1 type, 2 size, 3 info. Containers always come first.
pub fn sort_order(items: &[Item], column: i32, descending: bool, type_of: impl Fn(&Item) -> String) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..items.len()).collect();
    idx.sort_by(|&a, &b| {
        let (x, y) = (&items[a], &items[b]);
        let group = y.kind.is_container().cmp(&x.kind.is_container());
        let by_name = || x.name.to_lowercase().cmp(&y.name.to_lowercase());
        let ord = match column {
            1 => type_of(x).cmp(&type_of(y)).then_with(by_name),
            2 => x.size.cmp(&y.size).then_with(by_name),
            3 => x.info.cmp(&y.info).then_with(by_name),
            _ => by_name(),
        };
        group.then(if descending { ord.reverse() } else { ord })
    });
    idx
}

// ---------------------------------------------------------------------------
// Folder tree

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub vpath: String,
    pub depth: usize,
    pub kind: ItemKind,
    pub expandable: bool,
    pub expanded: bool,
}

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<TreeNode>,
}

fn containers(game_dir: &Path, vpath: &str, keys: Option<Arc<GtaVKeys>>, overlay: bool) -> Vec<(String, ItemKind)> {
    let Ok((listing, None)) = list(game_dir, vpath, keys, overlay) else { return Vec::new() };
    let mut out: Vec<(String, ItemKind)> =
        listing.items.into_iter().filter(|i| i.kind.is_container()).map(|i| (i.name, i.kind)).collect();
    out.sort_by(|a, b| {
        (a.1 != ItemKind::Dir).cmp(&(b.1 != ItemKind::Dir)).then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    out
}

impl Tree {
    pub fn build(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, overlay: bool) -> Self {
        let nodes = containers(game_dir, "", keys, overlay)
            .into_iter()
            .map(|(name, kind)| TreeNode {
                vpath: name.clone(),
                name,
                depth: 0,
                kind,
                expandable: true,
                expanded: false,
            })
            .collect();
        Self { nodes }
    }

    pub fn toggle(&mut self, idx: usize, game_dir: &Path, keys: Option<Arc<GtaVKeys>>, overlay: bool) {
        if self.nodes[idx].expanded { self.collapse(idx) } else { self.expand(idx, game_dir, keys, overlay) }
    }

    fn expand(&mut self, idx: usize, game_dir: &Path, keys: Option<Arc<GtaVKeys>>, overlay: bool) {
        let node = &self.nodes[idx];
        if node.expanded || !node.expandable {
            return;
        }
        let (depth, vpath) = (node.depth + 1, node.vpath.clone());
        let children: Vec<TreeNode> = containers(game_dir, &vpath, keys, overlay)
            .into_iter()
            .map(|(name, kind)| TreeNode {
                vpath: join(&vpath, &name),
                name,
                depth,
                kind,
                expandable: true,
                expanded: false,
            })
            .collect();
        let node = &mut self.nodes[idx];
        node.expandable = !children.is_empty();
        node.expanded = !children.is_empty();
        self.nodes.splice(idx + 1..idx + 1, children);
    }

    fn collapse(&mut self, idx: usize) {
        let depth = self.nodes[idx].depth;
        let end = self.nodes[idx + 1..].iter().position(|n| n.depth <= depth).map_or(self.nodes.len(), |p| idx + 1 + p);
        self.nodes.drain(idx + 1..end);
        self.nodes[idx].expanded = false;
    }

    /// Expands the ancestors of `vpath` and returns the node index for it, if shown.
    pub fn reveal(
        &mut self,
        vpath: &str,
        game_dir: &Path,
        keys: Option<Arc<GtaVKeys>>,
        overlay: bool,
    ) -> Option<usize> {
        let segments: Vec<&str> = vpath.split('/').filter(|s| !s.is_empty()).collect();
        let mut found = None;
        let mut prefix = String::new();
        for (i, seg) in segments.iter().enumerate() {
            prefix = join(&prefix, seg);
            let idx = self.nodes.iter().position(|n| n.vpath.eq_ignore_ascii_case(&prefix))?;
            found = Some(idx);
            if i + 1 < segments.len() {
                self.expand(idx, game_dir, keys.clone(), overlay);
            }
        }
        found
    }
}

// ---------------------------------------------------------------------------
// History

#[derive(Default)]
pub struct History {
    entries: Vec<String>,
    pos: usize,
}

impl History {
    pub fn push(&mut self, vpath: &str) {
        if self.entries.get(self.pos).is_some_and(|c| c == vpath) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.pos + 1);
        }
        self.entries.push(vpath.to_owned());
        self.pos = self.entries.len() - 1;
    }

    pub fn back(&mut self) -> Option<String> {
        (self.pos > 0).then(|| {
            self.pos -= 1;
            self.entries[self.pos].clone()
        })
    }

    pub fn forward(&mut self) -> Option<String> {
        (self.pos + 1 < self.entries.len()).then(|| {
            self.pos += 1;
            self.entries[self.pos].clone()
        })
    }

    pub fn can_back(&self) -> bool {
        self.pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers() {
        assert_eq!(normalize("\\x64a.rpf//data/"), "x64a.rpf/data");
        assert_eq!(parent("x64a.rpf/data").as_deref(), Some("x64a.rpf"));
        assert_eq!(parent("x64a.rpf").as_deref(), Some(""));
        assert_eq!(parent(""), None);
        assert_eq!(join("", "a"), "a");
        assert_eq!(shorten_front("a/b", 10), "a/b");
        assert_eq!(
            short_location("update/x64/dlcpacks/mpheist4/dlc.rpf/x64/data/lang/germandlc.rpf", 40),
            "mpheist4: x64/data/lang/germandlc.rpf"
        );
        assert_eq!(short_location("common.rpf/data/ai", 40), "common.rpf/data/ai");
        assert_eq!(short_location("update/x64/dlcpacks/mytest", 40), "mytest (DLC pack)");
        assert_eq!(
            shorten_front("update/x64/dlcpacks/mpbeach/dlc.rpf/common/data", 30),
            "…/mpbeach/dlc.rpf/common/data"
        );
    }

    #[test]
    fn history_navigation() {
        let mut h = History::default();
        h.push("");
        h.push("a");
        h.push("a/b");
        assert_eq!(h.back().as_deref(), Some("a"));
        h.push("c");
        assert!(!h.can_forward());
        assert_eq!(h.back().as_deref(), Some("a"));
        assert_eq!(h.back().as_deref(), Some(""));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward().as_deref(), Some("a"));
    }
}
