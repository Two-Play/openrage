//! Finds installed RAGE games in Steam libraries.

use std::fs;
use std::path::{Path, PathBuf};

use crate::setup::EXECUTABLES;

/// Steam install folder names of supported games.
/// GTA IV keeps its files in a `GTAIV` subfolder.
const GAME_FOLDERS: [&str; 3] = ["Grand Theft Auto V Enhanced", "Grand Theft Auto V", "Grand Theft Auto IV/GTAIV"];

/// Candidate Steam root directories for this platform.
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    if cfg!(windows) {
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        roots.push(PathBuf::from(r"C:\Program Files\Steam"));
    }
    roots
}

/// Library paths listed in a Steam `libraryfolders.vdf`.
fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').filter(|p| !p.trim().is_empty());
            if parts.next()? != "path" {
                return None;
            }
            Some(PathBuf::from(parts.next()?.replace("\\\\", "\\")))
        })
        .collect()
}

/// All GTA V and GTA IV installations found in Steam libraries, deduplicated.
pub fn find_installs() -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = Vec::new();
    for root in steam_roots() {
        let vdf = root.join("steamapps/libraryfolders.vdf");
        if let Ok(text) = fs::read_to_string(&vdf) {
            libraries.extend(library_paths(&text));
        }
        libraries.push(root);
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for lib in libraries {
        for folder in GAME_FOLDERS {
            let dir = lib.join("steamapps/common").join(folder);
            if is_gta5_dir(&dir) || crate::gta4::is_gta4_dir(&dir) {
                let canonical = dir.canonicalize().unwrap_or(dir);
                if !found.contains(&canonical) {
                    found.push(canonical);
                }
            }
        }
    }
    found
}

pub fn is_gta5_dir(dir: &Path) -> bool {
    EXECUTABLES.iter().any(|exe| dir.join(exe).is_file())
}

/// The program that marks a Red Dead Redemption 2 installation.
pub const RDR2_EXE: &str = "RDR2.exe";

/// Whether a folder is a Red Dead Redemption 2 installation.
pub fn is_rdr2_dir(dir: &Path) -> bool {
    dir.join(RDR2_EXE).is_file()
}

/// Short name of the game installed in `dir`: "GTA V Enhanced", "GTA V",
/// "GTA IV", "RDR2", or `None` when no known executable is there.
pub fn game_name(dir: &Path) -> Option<&'static str> {
    if is_rdr2_dir(dir) {
        Some("RDR2")
    } else if dir.join("GTA5_Enhanced.exe").is_file() {
        Some("GTA V Enhanced")
    } else if dir.join("GTA5.exe").is_file() {
        Some("GTA V")
    } else if crate::gta4::is_gta4_dir(dir) {
        Some("GTA IV")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_folders() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/user/.local/share/Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;
        assert_eq!(
            library_paths(vdf),
            [PathBuf::from("/home/user/.local/share/Steam"), PathBuf::from("D:\\SteamLibrary")]
        );
    }
}

/// Adds the archives (files with one of `extensions`) below `dir` to `out`,
/// sorted, skipping OpenRAGE's own folder (journal and kept originals are not
/// part of the game).
pub fn collect_archives(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(rage_mods::overlay::OPENRAGE_FOLDER)) {
            continue;
        }
        if p.is_dir() {
            collect_archives(&p, extensions, out);
        } else if p.extension().is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x))) {
            out.push(p);
        }
    }
}

/// [`collect_archives`] for `.rpf` files.
pub fn collect_rpfs(dir: &Path, out: &mut Vec<PathBuf>) {
    collect_archives(dir, &["rpf"], out);
}
