//! ASI mods of GTA V: the plugin loader, Script Hook V and the `.asi` files
//! next to the game, and on Linux the Proton setting that lets the loader run.
//!
//! ASI plugins are DLLs that a loader (a stand-in for a system DLL such as
//! `xinput1_4.dll` or `dinput8.dll`) loads when the game starts. On Windows the
//! game finds the loader in its folder by itself. Under Proton (Wine), Wine
//! uses its own copy of that system DLL unless the game's prefix overrides it
//! with "native, then builtin": without that no ASI is ever loaded.
//!
//! ASI mods are for story mode only: GTA Online bans modified games, and the
//! anti-cheat BattlEye that GTA V Enhanced starts with may stop loaders.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::journal::Journal;

/// Where disabled `.asi` files wait, inside the game folder.
const DISABLED: &str = "disabled-asi";

/// A plugin loader in the game folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loader {
    /// The DLL's file name, e.g. `xinput1_4.dll`.
    pub file: String,
    /// Its own description, e.g. "GTA V ENHANCED *.asi plugin loader".
    pub description: String,
    /// Built for GTA V Enhanced (true) or Legacy (false); `None` when it does
    /// not say (other loaders, such as Ultimate ASI Loader, work for both).
    pub for_enhanced: Option<bool>,
}

impl Loader {
    /// The DLL Wine must take from the game folder: the file name without `.dll`.
    pub fn dll(&self) -> String {
        self.file
            .strip_suffix(".dll")
            .or_else(|| self.file.strip_suffix(".DLL"))
            .unwrap_or(&self.file)
            .to_ascii_lowercase()
    }
}

/// An ASI plugin, enabled (in the game folder) or disabled (put aside by OpenRAGE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsiMod {
    pub name: String,
    pub enabled: bool,
    pub size: u64,
}

/// The Proton (Wine) prefix the game runs in, on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proton {
    pub app_id: String,
    /// The prefix folder (`…/steamapps/compatdata/<app id>/pfx`).
    pub prefix: PathBuf,
}

impl Proton {
    fn user_reg(&self) -> PathBuf {
        self.prefix.join("user.reg")
    }

    /// The DLL override set for `dll` in the prefix (e.g. "native,builtin").
    pub fn override_of(&self, dll: &str) -> Option<String> {
        let text = fs::read_to_string(self.user_reg()).ok()?;
        dll_override(&text, dll)
    }
}

/// Everything the ASI page shows about a game.
#[derive(Debug, Clone)]
pub struct Status {
    pub enhanced: bool,
    pub loaders: Vec<Loader>,
    pub script_hook: bool,
    pub mods: Vec<AsiMod>,
    /// `None` on Windows, or when the game does not run through Proton.
    pub proton: Option<Proton>,
    /// The game starts with BattlEye (GTA V Enhanced).
    pub battleye: bool,
}

impl Status {
    /// The loader that fits this edition of the game, if there is one.
    pub fn loader(&self) -> Option<&Loader> {
        self.loaders.iter().find(|l| l.for_enhanced == Some(self.enhanced)).or_else(|| {
            // A loader that does not say which edition it is for.
            self.loaders.iter().find(|l| l.for_enhanced.is_none())
        })
    }

    /// Whether Wine takes the fitting loader from the game folder (always
    /// true outside Proton).
    pub fn loader_runs(&self) -> bool {
        match (&self.proton, self.loader()) {
            (None, _) => true,
            (Some(p), Some(l)) => p.override_of(&l.dll()).is_some_and(|v| loads_native(&v)),
            (Some(_), None) => false,
        }
    }

    /// What stands between the game and its ASI mods, in plain words; empty
    /// when the mods load.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let edition = if self.enhanced { "GTA V Enhanced" } else { "GTA V Legacy" };
        match self.loader() {
            None if self.loaders.is_empty() => {
                out.push("No ASI loader in the game folder: install one (Script Hook V comes with one).".to_owned())
            }
            None => out.push(format!(
                "The ASI loader in the game folder ({}) is for the other edition of the game, not {edition}.",
                self.loaders.iter().map(|l| l.file.as_str()).collect::<Vec<_>>().join(", ")
            )),
            Some(l) if !self.loader_runs() => out.push(format!(
                "Proton uses its own {} instead of the loader in the game folder: no ASI is loaded. \
                 Let Proton load it (or add WINEDLLOVERRIDES=\"{}=n,b\" %command% to the game's launch options in Steam).",
                l.file,
                l.dll()
            )),
            Some(_) => {}
        }
        if !self.script_hook && self.mods.iter().any(|m| m.enabled) {
            out.push("Script Hook V (ScriptHookV.dll) is missing: most ASI mods need it.".to_owned());
        }
        out
    }
}

/// Whether a DLL override value makes Wine try the game's own DLL first.
fn loads_native(value: &str) -> bool {
    value.split(',').next().is_some_and(|first| matches!(first.trim(), "native" | "n"))
}

/// Reads the facts about the ASI mods of a GTA V folder.
pub fn status(game_dir: &Path) -> Result<Status> {
    if !crate::detect::is_gta5_dir(game_dir) {
        bail!("{} is not a GTA V folder: ASI mods are for GTA V", game_dir.display());
    }
    let enhanced = game_dir.join("GTA5_Enhanced.exe").is_file();
    let mut loaders = Vec::new();
    let mut mods = Vec::new();
    let mut script_hook = false;
    for entry in fs::read_dir(game_dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if lower == "scripthookv.dll" {
            script_hook = true;
        } else if lower.ends_with(".asi") {
            mods.push(AsiMod { name, enabled: true, size: meta.len() });
        } else if lower.ends_with(".dll")
            && meta.len() < 16 << 20
            && let Ok(data) = fs::read(entry.path())
            && let Some(loader) = identify_loader(&name, &data)
        {
            loaders.push(loader);
        }
    }
    if let Ok(entries) = fs::read_dir(disabled_dir(game_dir)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".asi")
                && let Ok(meta) = entry.metadata()
            {
                mods.push(AsiMod { name, enabled: false, size: meta.len() });
            }
        }
    }
    mods.sort_by_key(|m| m.name.to_ascii_lowercase());
    loaders.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Status {
        enhanced,
        loaders,
        script_hook,
        mods,
        proton: if cfg!(windows) { None } else { find_proton(game_dir) },
        battleye: game_dir.join("GTA5_Enhanced_BE.exe").is_file(),
    })
}

/// A DLL that loads `.asi` plugins, recognised by what it says about itself
/// (the file name alone says nothing: loaders pose as system DLLs).
fn identify_loader(file: &str, data: &[u8]) -> Option<Loader> {
    let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
    let contains = |needle: &[u8]| memchr::memmem::find(data, needle).is_some();
    let ultimate = contains(b"Ultimate ASI Loader") || contains(&utf16("Ultimate ASI Loader"));
    let scripthook_loader = contains(b"LOADER: Loading *.asi plugins");
    if !ultimate && !scripthook_loader {
        return None;
    }
    let description = if ultimate {
        "Ultimate ASI Loader".to_owned()
    } else if contains(&utf16("GTA V ENHANCED *.asi plugin loader")) {
        "GTA V Enhanced *.asi plugin loader".to_owned()
    } else {
        "GTA V *.asi plugin loader".to_owned()
    };
    let for_enhanced = (!ultimate).then(|| description.contains("Enhanced"));
    Some(Loader { file: file.to_owned(), description, for_enhanced })
}

fn disabled_dir(game_dir: &Path) -> PathBuf {
    game_dir.join(rage_mods::overlay::OPENRAGE_FOLDER).join(DISABLED)
}

/// A plain `.asi` file name (no folders): anything else is refused.
fn checked_name(name: &str) -> Result<&str> {
    let ok = name.to_ascii_lowercase().ends_with(".asi")
        && !name.contains(['/', '\\', ':'])
        && !name.starts_with('.')
        && !name.chars().any(char::is_control);
    if !ok {
        bail!("{name:?} is not the name of an .asi file");
    }
    Ok(name)
}

/// Turns an ASI mod on (back into the game folder) or off (aside, into
/// OpenRAGE's folder, so the loader skips it). Nothing is deleted.
pub fn set_enabled(game_dir: &Path, name: &str, enabled: bool) -> Result<()> {
    let name = checked_name(name)?;
    let (on, off) = (game_dir.join(name), disabled_dir(game_dir).join(name));
    let (from, to) = if enabled { (off, on) } else { (on, off) };
    if !from.is_file() {
        bail!("{name} is not {}", if enabled { "disabled" } else { "in the game folder" });
    }
    if to.exists() {
        bail!("{} already exists", to.display());
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::rename(&from, &to).with_context(|| format!("moving {name}"))?;
    Ok(())
}

/// Copies an ASI mod into the game folder. Recorded in the history, so undo
/// removes it again.
pub fn install(game_dir: &Path, file: &Path) -> Result<PathBuf> {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).context("no file name")?;
    let name = checked_name(&name)?.to_owned();
    if game_dir.join(&name).exists() || disabled_dir(game_dir).join(&name).exists() {
        bail!("{name} is already installed");
    }
    let data = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    if !data.starts_with(b"MZ") {
        bail!("{name} is not a Windows DLL");
    }
    let mods = crate::install::mods_dir_or_create(game_dir)?;
    Journal::new(&mods).write_file(true, &name, &data, &format!("ASI mod {name} installed"))
}

/// The Proton prefix of a game installed through Steam: the app whose
/// `installdir` is the game's folder, in the same Steam library.
fn find_proton(game_dir: &Path) -> Option<Proton> {
    let dir_name = game_dir.file_name()?.to_string_lossy().into_owned();
    let common = game_dir.parent()?;
    if !common.file_name()?.to_string_lossy().eq_ignore_ascii_case("common") {
        return None;
    }
    let steamapps = common.parent()?;
    for entry in fs::read_dir(steamapps).ok()?.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        if acf_value(&text, "installdir").is_some_and(|d| d.eq_ignore_ascii_case(&dir_name))
            && let Some(app_id) = acf_value(&text, "appid")
        {
            let prefix = steamapps.join("compatdata").join(&app_id).join("pfx");
            return prefix.join("user.reg").is_file().then_some(Proton { app_id, prefix });
        }
    }
    None
}

/// A top-level `"key" "value"` of a Steam `.acf` file.
fn acf_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split('"').filter(|p| !p.trim().is_empty());
        (parts.next()? == key).then(|| parts.next().map(str::to_owned)).flatten()
    })
}

const OVERRIDES: &str = "[Software\\\\Wine\\\\DllOverrides]";

/// The value of a DLL override in a Wine `user.reg`.
fn dll_override(reg: &str, dll: &str) -> Option<String> {
    let mut in_section = false;
    for line in reg.lines() {
        if line.starts_with('[') {
            in_section = line.starts_with(OVERRIDES);
            continue;
        }
        if in_section
            && let Some(rest) = line.strip_prefix('"')
            && let Some((key, value)) = rest.split_once("\"=\"")
            && key.eq_ignore_ascii_case(dll)
        {
            return Some(value.trim_end_matches('"').to_owned());
        }
    }
    None
}

/// `user.reg` with the override for `dll` set to `value` (`None`: removed).
fn with_override(reg: &str, dll: &str, value: Option<&str>) -> String {
    let line_for = |v: &str| format!("\"{dll}\"=\"{v}\"");
    let mut out: Vec<String> = Vec::new();
    let (mut in_section, mut found_section, mut done) = (false, false, false);
    for line in reg.lines() {
        if line.starts_with('[') {
            if in_section
                && !done
                && let Some(v) = value
            {
                insert_before_blank(&mut out, line_for(v));
                done = true;
            }
            in_section = line.starts_with(OVERRIDES);
            found_section |= in_section;
        } else if in_section
            && let Some(rest) = line.strip_prefix('"')
            && rest.split_once("\"=\"").is_some_and(|(key, _)| key.eq_ignore_ascii_case(dll))
        {
            if let Some(v) = value
                && !done
            {
                out.push(line_for(v));
                done = true;
            }
            continue;
        }
        out.push(line.to_owned());
    }
    if let Some(v) = value
        && !done
    {
        if !found_section {
            out.push(String::new());
            out.push(OVERRIDES.to_owned());
        }
        insert_before_blank(&mut out, line_for(v));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Adds a line at the end of the section being written, before its blank lines.
fn insert_before_blank(out: &mut Vec<String>, line: String) {
    let at = out.iter().rposition(|l| !l.trim().is_empty()).map_or(out.len(), |i| i + 1);
    out.insert(at, line);
}

/// Whether the game or its prefix is in use: Wine rewrites `user.reg` when it
/// exits, so changing it then would be undone.
fn prefix_in_use(proton: &Proton) -> bool {
    let needle = format!("compatdata/{}/", proton.app_id);
    let Ok(procs) = fs::read_dir("/proc") else { return false };
    procs.flatten().any(|p| {
        let Ok(cmd) = fs::read(p.path().join("cmdline")) else { return false };
        let cmd = String::from_utf8_lossy(&cmd).to_ascii_lowercase();
        cmd.contains(&needle) || ["gta5_enhanced.exe", "gta5.exe", "playgtav.exe"].iter().any(|e| cmd.contains(e))
    })
}

/// Lets Proton load the game's own `dll` (the ASI loader) instead of Wine's
/// (`on`), or takes that back. `user.reg` is backed up first (once, as
/// `user.reg.openrage-backup`).
pub fn set_proton_override(proton: &Proton, dll: &str, on: bool) -> Result<()> {
    if dll.is_empty() || !dll.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        bail!("{dll:?} is not a DLL name");
    }
    if prefix_in_use(proton) {
        bail!("the game is running: close it first (Wine would undo the change when it exits)");
    }
    let reg = proton.user_reg();
    let text = fs::read_to_string(&reg).with_context(|| format!("reading {}", reg.display()))?;
    let backup = proton.prefix.join("user.reg.openrage-backup");
    if !backup.exists() {
        fs::copy(&reg, &backup).with_context(|| format!("backing up {}", reg.display()))?;
    }
    let changed = with_override(&text, dll, on.then_some("native,builtin"));
    let tmp = proton.prefix.join("user.reg.openrage-tmp");
    fs::write(&tmp, changed)?;
    fs::rename(&tmp, &reg).with_context(|| format!("writing {}", reg.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &str = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\AppDefaults\\\\x.exe\\\\DllOverrides] 1\n\"dinput8\"=\"builtin\"\n\n[Software\\\\Wine\\\\DllOverrides] 1760544893\n#time=1dc3deed75fdde0\n\"atl100\"=\"native,builtin\"\n\"d3d11\"=\"native\"\n\n[Software\\\\Wine\\\\Fonts] 1\n\"x\"=\"y\"\n";

    #[test]
    fn reads_and_writes_dll_overrides() {
        assert_eq!(dll_override(REG, "atl100").as_deref(), Some("native,builtin"));
        assert_eq!(dll_override(REG, "dinput8"), None, "only the global section counts");
        let on = with_override(REG, "xinput1_4", Some("native,builtin"));
        assert_eq!(dll_override(&on, "xinput1_4").as_deref(), Some("native,builtin"));
        assert!(
            on.contains("\"d3d11\"=\"native\"\n\"xinput1_4\"=\"native,builtin\"\n\n[Software\\\\Wine\\\\Fonts]"),
            "{on}"
        );
        // Setting it again replaces the line; removing it gives the original back.
        let again = with_override(&on, "XINPUT1_4", Some("native,builtin"));
        assert_eq!(again.matches("xinput1_4").count() + again.matches("XINPUT1_4").count(), 1, "{again}");
        assert_eq!(with_override(&on, "xinput1_4", None), REG);
        // A prefix without the section gets one.
        let fresh = with_override("WINE REGISTRY Version 2\n", "dinput8", Some("native,builtin"));
        assert_eq!(dll_override(&fresh, "dinput8").as_deref(), Some("native,builtin"));
        assert!(loads_native("native,builtin") && loads_native("n,b") && !loads_native("builtin"));
    }

    #[test]
    fn recognises_loaders_by_their_own_words() {
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let mut enhanced = b"MZ....LOADER: Loading *.asi plugins".to_vec();
        enhanced.extend(utf16("GTA V ENHANCED *.asi plugin loader"));
        let l = identify_loader("xinput1_4.dll", &enhanced).unwrap();
        assert_eq!((l.for_enhanced, l.dll().as_str()), (Some(true), "xinput1_4"));
        let legacy = identify_loader("dinput8.dll", b"MZ..LOADER: Loading *.asi plugins").unwrap();
        assert_eq!(legacy.for_enhanced, Some(false));
        assert_eq!(identify_loader("version.dll", b"MZ..Ultimate ASI Loader").unwrap().for_enhanced, None);
        assert!(identify_loader("d3d11.dll", b"MZ..something else").is_none());
    }

    fn fake_install(name: &str, enhanced: bool) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("openrage-asi-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let game = root.join("steamapps/common/Grand Theft Auto V Enhanced");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join(if enhanced { "GTA5_Enhanced.exe" } else { "GTA5.exe" }), b"MZ").unwrap();
        fs::write(game.join("dinput8.dll"), b"MZ..LOADER: Loading *.asi plugins").unwrap();
        fs::write(game.join("NativeTrainer.asi"), b"MZ trainer").unwrap();
        fs::write(game.join("ScriptHookV.dll"), b"MZ").unwrap();
        fs::write(
            root.join("steamapps/appmanifest_3240220.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"3240220\"\n\t\"installdir\"\t\t\"Grand Theft Auto V Enhanced\"\n}\n",
        )
        .unwrap();
        let pfx = root.join("steamapps/compatdata/3240220/pfx");
        fs::create_dir_all(&pfx).unwrap();
        fs::write(pfx.join("user.reg"), REG).unwrap();
        (root, game)
    }

    #[test]
    fn explains_why_mods_do_not_load_and_fixes_it() {
        let (root, game) = fake_install("status", true);
        let s = status(&game).unwrap();
        assert!(s.enhanced && s.script_hook);
        assert_eq!(s.mods, vec![AsiMod { name: "NativeTrainer.asi".into(), enabled: true, size: 10 }]);
        // Only the Legacy loader: it does not fit Enhanced.
        assert!(s.loader().is_none());
        assert!(s.problems()[0].contains("other edition"), "{:?}", s.problems());
        // With the Enhanced loader, Proton is what stops it.
        let utf16: Vec<u8> = "GTA V ENHANCED *.asi plugin loader".encode_utf16().flat_map(u16::to_le_bytes).collect();
        fs::write(game.join("xinput1_4.dll"), [b"MZ..LOADER: Loading *.asi plugins".as_slice(), &utf16].concat())
            .unwrap();
        let s = status(&game).unwrap();
        assert_eq!(s.loader().unwrap().file, "xinput1_4.dll");
        if cfg!(windows) {
            // No Proton on Windows: the game loads the loader by itself.
            assert!(s.proton.is_none());
            assert!(s.loader_runs() && s.problems().is_empty(), "{:?}", s.problems());
        } else {
            let proton = s.proton.clone().expect("prefix found through the app manifest");
            assert!(!s.loader_runs());
            assert!(s.problems()[0].contains("xinput1_4=n,b"), "{:?}", s.problems());
            set_proton_override(&proton, "xinput1_4", true).unwrap();
            let s = status(&game).unwrap();
            assert!(s.loader_runs() && s.problems().is_empty(), "{:?}", s.problems());
            assert!(proton.prefix.join("user.reg.openrage-backup").is_file());
            set_proton_override(&proton, "xinput1_4", false).unwrap();
            assert_eq!(fs::read_to_string(proton.prefix.join("user.reg")).unwrap(), REG);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mods_are_switched_off_and_on_and_installed_with_undo() {
        let (root, game) = fake_install("switch", false);
        set_enabled(&game, "NativeTrainer.asi", false).unwrap();
        assert!(!game.join("NativeTrainer.asi").exists());
        assert!(!status(&game).unwrap().mods[0].enabled);
        set_enabled(&game, "NativeTrainer.asi", true).unwrap();
        assert!(game.join("NativeTrainer.asi").is_file());
        for bad in ["../x.asi", "x.dll", ".hidden.asi"] {
            assert!(set_enabled(&game, bad, false).is_err(), "{bad}");
        }
        let new = root.join("Speedo.asi");
        fs::write(&new, b"MZ speedo").unwrap();
        install(&game, &new).unwrap();
        assert!(game.join("Speedo.asi").is_file());
        assert!(install(&game, &new).is_err(), "installed twice");
        crate::install::undo_last(&game).unwrap();
        assert!(!game.join("Speedo.asi").exists(), "undo removes it");
        fs::write(root.join("fake.asi"), b"not a dll").unwrap();
        assert!(install(&game, &root.join("fake.asi")).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
