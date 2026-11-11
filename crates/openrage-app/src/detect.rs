//! Finds installed RAGE games in Steam libraries.

use std::fs;
use std::path::{Path, PathBuf};

use crate::setup::EXECUTABLES;

/// Steam install folder names of supported games.
const GAME_FOLDERS: [&str; 2] = ["Grand Theft Auto V Enhanced", "Grand Theft Auto V"];

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

/// All GTA V installations found in Steam libraries, deduplicated.
pub fn find_gta5_installs() -> Vec<PathBuf> {
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
            if is_gta5_dir(&dir) {
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

/// Short name of the game installed in `dir`: "GTA V Enhanced", "GTA V",
/// "RDR2", or `None` when no known executable is there.
pub fn game_name(dir: &Path) -> Option<&'static str> {
    if dir.join("RDR2.exe").is_file() {
        Some("RDR2")
    } else if dir.join("GTA5_Enhanced.exe").is_file() {
        Some("GTA V Enhanced")
    } else if dir.join("GTA5.exe").is_file() {
        Some("GTA V")
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
