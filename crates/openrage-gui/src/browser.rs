//! UI-independent browsing logic: folder listings, the folder tree and history.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_crypto::GtaVKeys;
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, EntryKind, Rpf7, RpfError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Dir,
    Archive,
    File,
    Resource,
}

impl ItemKind {
    pub fn is_container(self) -> bool {
        matches!(self, Self::Dir | Self::Archive)
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
}

/// Where a listing comes from.
#[derive(Clone)]
pub enum Location {
    Disk(PathBuf),
    Archive(Arc<Rpf7>, EntryId),
    /// Results of a search; `summary` describes hash lookups etc.
    Search {
        query: String,
        summary: String,
    },
}

/// Virtual paths starting with `?` are searches.
pub fn search_query(vpath: &str) -> Option<&str> {
    vpath.strip_prefix('?').map(str::trim).filter(|q| !q.is_empty())
}

pub struct Listing {
    /// Normalized virtual path (`/`-separated, no leading or trailing slash).
    pub vpath: String,
    pub location: Location,
    pub items: Vec<Item>,
}

pub fn normalize(vpath: &str) -> String {
    if vpath.starts_with('?') {
        return vpath.trim().to_owned();
    }
    vpath.replace('\\', "/").split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

pub fn parent(vpath: &str) -> Option<String> {
    if vpath.is_empty() || vpath.starts_with('?') {
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
pub fn list(game_dir: &Path, vpath: &str, keys: Option<Arc<GtaVKeys>>) -> Result<(Listing, Option<String>), RpfError> {
    let vpath = normalize(vpath);
    match vfs::resolve(game_dir, &vpath, keys.clone())? {
        Node::Dir(dir) => Ok((list_disk(vpath, dir)?, None)),
        Node::ArchiveDir(rpf, dir) => Ok((list_archive(vpath, rpf, dir), None)),
        Node::ArchiveFile(..) | Node::PlainFile(_) => {
            let name = last_segment(&vpath);
            let (listing, _) = list(game_dir, &parent(&vpath).unwrap_or_default(), keys)?;
            Ok((listing, Some(name)))
        }
    }
}

fn list_disk(vpath: String, dir: PathBuf) -> Result<Listing, RpfError> {
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
        items.push(Item { name, kind, size, info: String::new(), entry: None, vpath: None });
    }
    Ok(Listing { vpath, location: Location::Disk(dir), items })
}

fn list_archive(vpath: String, rpf: Arc<Rpf7>, dir: EntryId) -> Listing {
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
            Item { name: e.name.clone(), kind, size: e.size(), info, entry: Some(id), vpath: None }
        })
        .collect();
    Listing { vpath, location: Location::Archive(rpf, dir), items }
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

fn containers(game_dir: &Path, vpath: &str, keys: Option<Arc<GtaVKeys>>) -> Vec<(String, ItemKind)> {
    let Ok((listing, None)) = list(game_dir, vpath, keys) else { return Vec::new() };
    let mut out: Vec<(String, ItemKind)> =
        listing.items.into_iter().filter(|i| i.kind.is_container()).map(|i| (i.name, i.kind)).collect();
    out.sort_by(|a, b| {
        (a.1 != ItemKind::Dir).cmp(&(b.1 != ItemKind::Dir)).then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    out
}

impl Tree {
    pub fn build(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Self {
        let nodes = containers(game_dir, "", keys)
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

    pub fn toggle(&mut self, idx: usize, game_dir: &Path, keys: Option<Arc<GtaVKeys>>) {
        if self.nodes[idx].expanded { self.collapse(idx) } else { self.expand(idx, game_dir, keys) }
    }

    fn expand(&mut self, idx: usize, game_dir: &Path, keys: Option<Arc<GtaVKeys>>) {
        let node = &self.nodes[idx];
        if node.expanded || !node.expandable {
            return;
        }
        let (depth, vpath) = (node.depth + 1, node.vpath.clone());
        let children: Vec<TreeNode> = containers(game_dir, &vpath, keys)
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
    pub fn reveal(&mut self, vpath: &str, game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Option<usize> {
        let segments: Vec<&str> = vpath.split('/').filter(|s| !s.is_empty()).collect();
        let mut found = None;
        let mut prefix = String::new();
        for (i, seg) in segments.iter().enumerate() {
            prefix = join(&prefix, seg);
            let idx = self.nodes.iter().position(|n| n.vpath.eq_ignore_ascii_case(&prefix))?;
            found = Some(idx);
            if i + 1 < segments.len() {
                self.expand(idx, game_dir, keys.clone());
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
