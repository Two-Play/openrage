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

/// The program in the GTA VI Scarlett build.
pub const GTA6_EXE: &str = "game_scarlett_master.exe";

/// Whether a folder holds GTA VI: its Scarlett executable or archives in the
/// RPF9 format. Archives can also be explored without the executable through
/// the recovery view (see [`rage_rpf::rpf9`]).
pub fn is_gta6_dir(dir: &Path) -> bool {
    if is_gta5_dir(dir) || is_rdr2_dir(dir) || crate::gta4::is_gta4_dir(dir) {
        return false;
    }
    if dir.join(GTA6_EXE).is_file() || rage_rpf::Rpf9::file_is_rpf9(&dir.join("common.rpf")) {
        return true;
    }
    let Ok(rd) = fs::read_dir(dir) else { return false };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")))
        .any(|p| rage_rpf::Rpf9::file_is_rpf9(&p))
}

/// Whether a folder holds a game OpenRAGE can open.
pub fn is_game_dir(dir: &Path) -> bool {
    is_gta5_dir(dir) || is_rdr2_dir(dir) || crate::gta4::is_gta4_dir(dir) || is_gta6_dir(dir)
}

/// The icon of the game in `dir`, from its own program: (width, height, RGBA).
pub fn game_icon(dir: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let programs: &[&str] = if is_rdr2_dir(dir) {
        &[RDR2_EXE]
    } else if crate::gta4::is_gta4_dir(dir) {
        &["GTAIV.exe"]
    } else if is_gta6_dir(dir) {
        &[GTA6_EXE]
    } else {
        &["GTA5_Enhanced.exe", "GTA5.exe", "PlayGTAV.exe"]
    };
    programs.iter().map(|p| dir.join(p)).filter(|p| p.is_file()).find_map(|p| crate::exe_icon::icon(&p))
}

/// Short name of the game installed in `dir`: "GTA V Enhanced", "GTA V",
/// "GTA IV", "RDR2", "GTA VI", or `None` when the folder holds no known game.
pub fn game_name(dir: &Path) -> Option<&'static str> {
    if is_rdr2_dir(dir) {
        Some("RDR2")
    } else if dir.join("GTA5_Enhanced.exe").is_file() {
        Some("GTA V Enhanced")
    } else if dir.join("GTA5.exe").is_file() {
        Some("GTA V")
    } else if crate::gta4::is_gta4_dir(dir) {
        Some("GTA IV")
    } else if is_gta6_dir(dir) {
        Some("GTA VI")
    } else {
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gta6_from_scarlett_executable_or_rpf9_archives() {
        let dir = std::env::temp_dir().join(format!("openrage-detect-gta6-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(!is_game_dir(&dir));

        // The executable identifies the game even before archives are copied.
        fs::write(dir.join(GTA6_EXE), b"MZ").unwrap();
        assert!(is_gta6_dir(&dir));
        assert!(is_game_dir(&dir));
        assert_eq!(game_name(&dir), Some("GTA VI"));
        fs::remove_file(dir.join(GTA6_EXE)).unwrap();

        // Archive-only copies remain supported. A non-RPF9 common.rpf must
        // not hide another archive with the RPF9 signature.
        fs::write(dir.join("common.rpf"), b"unrelated data").unwrap();
        assert!(!is_gta6_dir(&dir));
        fs::write(dir.join("data.RPF"), b"9FPR").unwrap();
        assert!(is_gta6_dir(&dir));
        assert_eq!(game_name(&dir), Some("GTA VI"));

        // An RPF9 file copied into another game's folder does not switch it.
        fs::write(dir.join("GTA5.exe"), b"MZ").unwrap();
        assert!(!is_gta6_dir(&dir));
        assert_eq!(game_name(&dir), Some("GTA V"));
        fs::remove_dir_all(&dir).unwrap();
    }

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
