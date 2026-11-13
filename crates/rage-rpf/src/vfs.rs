//! A virtual file system that treats archives (and archives inside archives)
//! as ordinary directories.
//!
//! Virtual paths use `/` or `\` and may cross archive boundaries, e.g.
//! `update/update.rpf/x64/data/lang/americandlc.rpf/global.gxt2`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_crypto::GtaVKeys;

#[cfg(feature = "rdr2")]
use crate::Rpf8;
use crate::{EntryId, Gta4Archive, Gta4EntryId, Rpf7, RpfError};

/// What a virtual path points to. More variants appear with crate features
/// (`rdr2`), so matches need a fallback arm.
#[non_exhaustive]
pub enum Node {
    /// A real directory on disk.
    Dir(PathBuf),
    /// A real non-archive file on disk.
    PlainFile(PathBuf),
    /// A directory inside an archive (the archive root for a bare `.rpf`).
    ArchiveDir(Arc<Rpf7>, EntryId),
    /// A file inside an archive.
    ArchiveFile(Arc<Rpf7>, EntryId),
    /// A folder inside an RDR2 archive ("" for its top): RPF8 archives have no
    /// directories of their own, folders come from the entries' paths.
    #[cfg(feature = "rdr2")]
    Rpf8Dir(Arc<Rpf8>, String),
    /// A file inside an RDR2 archive (index into its entries).
    #[cfg(feature = "rdr2")]
    Rpf8File(Arc<Rpf8>, usize),
    /// A folder inside a GTA IV archive (`.img`, RPF2/RPF3); the archive root
    /// is [`Gta4Archive::ROOT`].
    Gta4Dir(Arc<Gta4Archive>, Gta4EntryId),
    /// A file inside a GTA IV archive.
    Gta4File(Arc<Gta4Archive>, Gta4EntryId),
}

fn is_rpf(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".rpf")
}

/// Resolves `vpath` relative to `root` (or as an absolute path).
pub fn resolve(root: &Path, vpath: &str, keys: Option<Arc<GtaVKeys>>) -> Result<Node, RpfError> {
    let normalized = vpath.replace('\\', "/");
    let full = if Path::new(&normalized).is_absolute() { PathBuf::from(&normalized) } else { root.join(&normalized) };

    // Walk components on disk, matching case-insensitively (game files on
    // Linux keep their Windows casing), until we hit a file or run out.
    let mut components = full.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
    let mut phys = PathBuf::new();
    let mut archive: Option<Arc<Rpf7>> = None;
    for comp in components.by_ref() {
        let next = match_case_insensitive(&phys, &comp).ok_or_else(|| RpfError::NotFound(vpath.to_owned()))?;
        if next.is_dir() {
            phys = next;
            continue;
        }
        if crate::gta4::is_archive_name(&comp) && Gta4Archive::file_is_gta4(&next) {
            let rest: Vec<String> = components.collect();
            return resolve_gta4(Arc::new(Gta4Archive::open_default(&next)?), &rest, vpath);
        }
        if is_rpf(&comp) {
            #[cfg(feature = "rdr2")]
            if Rpf8::file_is_rpf8(&next) {
                let rest: Vec<String> = components.collect();
                return resolve_rpf8(Arc::new(Rpf8::open_default(&next)?), &rest, vpath);
            }
            archive = Some(Arc::new(Rpf7::open(&next, keys.clone())?));
            break;
        }
        return match components.next() {
            None => Ok(Node::PlainFile(next)),
            Some(_) => Err(RpfError::NotFound(vpath.to_owned())),
        };
    }

    let Some(mut archive) = archive else { return Ok(Node::Dir(phys)) };
    let mut cur = Rpf7::ROOT;
    let rest: Vec<String> = components.collect();
    for (i, comp) in rest.iter().enumerate() {
        let child = archive.find_child(cur, comp).ok_or_else(|| RpfError::NotFound(vpath.to_owned()))?;
        let entry = archive.entry(child);
        if entry.is_dir() {
            cur = child;
        } else if entry.is_archive() {
            archive = Arc::new(archive.open_child(child)?);
            cur = Rpf7::ROOT;
        } else if i + 1 == rest.len() {
            return Ok(Node::ArchiveFile(archive, child));
        } else {
            return Err(RpfError::NotFound(vpath.to_owned()));
        }
    }
    Ok(Node::ArchiveDir(archive, cur))
}

#[cfg(feature = "rdr2")]
/// Walks the rest of a path inside an RDR2 archive: folders come from entry
/// paths, nested archives are entered.
fn resolve_rpf8(mut archive: Arc<Rpf8>, rest: &[String], vpath: &str) -> Result<Node, RpfError> {
    if archive.entries().is_none() {
        return Err(RpfError::KeysRequired(archive.display_name().to_owned()));
    }
    let mut dir = String::new();
    for (i, comp) in rest.iter().enumerate() {
        let path = if dir.is_empty() { comp.clone() } else { format!("{dir}/{comp}") };
        if let Some(idx) = archive.find(&path) {
            let entry = &archive.entries().expect("entries")[idx];
            if entry.ext_id == 0 {
                archive = Arc::new(archive.open_child(entry)?);
                dir.clear();
                continue;
            }
            if i + 1 == rest.len() {
                return Ok(Node::Rpf8File(archive, idx));
            }
            return Err(RpfError::NotFound(vpath.to_owned()));
        }
        if archive.is_dir(&path) {
            dir = path;
        } else {
            return Err(RpfError::NotFound(vpath.to_owned()));
        }
    }
    Ok(Node::Rpf8Dir(archive, dir))
}

/// Walks the rest of a path inside a GTA IV archive.
fn resolve_gta4(archive: Arc<Gta4Archive>, rest: &[String], vpath: &str) -> Result<Node, RpfError> {
    let mut cur = Gta4Archive::ROOT;
    for (i, comp) in rest.iter().enumerate() {
        let child = archive.find_child(cur, comp).ok_or_else(|| RpfError::NotFound(vpath.to_owned()))?;
        if archive.entry(child).is_dir() {
            cur = child;
        } else if i + 1 == rest.len() {
            return Ok(Node::Gta4File(archive, child));
        } else {
            return Err(RpfError::NotFound(vpath.to_owned()));
        }
    }
    Ok(Node::Gta4Dir(archive, cur))
}

/// Depth-first walk over a folder inside a GTA IV archive; `visit` receives
/// each entry and its path relative to the walk start.
pub fn walk_gta4(archive: &Gta4Archive, dir: Gta4EntryId, prefix: &str, visit: &mut dyn FnMut(Gta4EntryId, &str)) {
    for child in archive.children(dir) {
        let entry = archive.entry(child);
        let path = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
        visit(child, &path);
        if entry.is_dir() {
            walk_gta4(archive, child, &path, visit);
        }
    }
}

/// Finds `name` inside `dir`, ignoring ASCII case (game files keep Windows
/// casing on Linux). Returns the entry with its casing *on disk*, also on
/// case-insensitive file systems, preferring an exact match.
pub fn match_case_insensitive(dir: &Path, name: &str) -> Option<PathBuf> {
    let listing_dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let direct = || {
        let p = if dir.as_os_str().is_empty() { PathBuf::from(name) } else { dir.join(name) };
        p.exists().then_some(p)
    };
    // Special components (root, prefixes, `..`) are not directory entries.
    if name.is_empty() || name == "." || name == ".." || name.contains([':', '/', '\\']) {
        return direct();
    }
    let Ok(entries) = std::fs::read_dir(listing_dir) else { return direct() };
    let mut fallback = None;
    for e in entries.flatten() {
        let file_name = e.file_name();
        let entry_name = file_name.to_string_lossy();
        if entry_name == name {
            return Some(dir.join(&file_name));
        }
        if fallback.is_none() && entry_name.eq_ignore_ascii_case(name) {
            fallback = Some(dir.join(&file_name));
        }
    }
    fallback.or_else(direct)
}

/// Resolves a `/`-separated relative path below `root` case-insensitively.
pub fn find_case_insensitive(root: &Path, rel: &str) -> Option<PathBuf> {
    rel.split('/').filter(|s| !s.is_empty()).try_fold(root.to_path_buf(), |dir, seg| match_case_insensitive(&dir, seg))
}

/// Depth-first walk over a directory inside an archive.
///
/// `visit` receives the archive, the entry and the entry's path relative to the
/// walk start. With `into_archives`, nested `.rpf` entries are opened and walked
/// as well; failures to open them are reported via `on_error` and skipped.
pub fn walk(
    archive: &Rpf7,
    dir: EntryId,
    prefix: &str,
    into_archives: bool,
    visit: &mut dyn FnMut(&Rpf7, EntryId, &str),
    on_error: &mut dyn FnMut(RpfError),
) {
    for child in archive.children(dir) {
        let entry = archive.entry(child);
        let path = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
        visit(archive, child, &path);
        if entry.is_dir() {
            walk(archive, child, &path, into_archives, visit, on_error);
        } else if into_archives && entry.is_archive() {
            match archive.open_child(child) {
                Ok(inner) => walk(&inner, Rpf7::ROOT, &path, into_archives, visit, on_error),
                Err(e) => on_error(e),
            }
        }
    }
}

/// Turns an archive-internal path into a relative file-system path, or `None`
/// if it could escape the extraction directory. Archive contents are untrusted
/// (mods are shared freely), so every extraction must go through this.
pub fn safe_relative_path(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in rel.split(['/', '\\']) {
        let bad = part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(':')
            || part.chars().any(|c| c.is_control());
        if bad {
            return None;
        }
        out.push(part);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::safe_relative_path;

    #[test]
    fn rejects_escaping_paths() {
        assert!(safe_relative_path("data/x.ymt").is_some());
        for bad in ["", "../x", "a/../../x", "/etc/passwd", "a//b", "C:/x", "a\\..\\b", "a/\u{0}b"] {
            assert!(safe_relative_path(bad).is_none(), "{bad:?} must be rejected");
        }
    }
}
