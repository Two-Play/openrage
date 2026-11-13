//! Text, script and XML files: opening, editing and exporting them.

use crate::*;

/// Reads a file as the game sees it (mods included).
pub(crate) fn read_file(ui: &AppWindow, vpath: &str) -> anyhow::Result<Vec<u8>> {
    let _ = ui;
    let (game_dir, keys) = with_app(|a| (a.game_dir.clone(), a.keys.clone()));
    let game_dir = game_dir.ok_or_else(|| anyhow::anyhow!("no game folder"))?;
    match rage_mods::resolve(&game_dir, vpath, keys)?.0 {
        Node::ArchiveFile(rpf, id) => Ok(rpf.read(id)?),
        Node::PlainFile(p) => Ok(std::fs::read(p)?),
        Node::Rpf8File(rpf, i) => {
            let entry =
                rpf.entries().and_then(|e| e.get(i)).ok_or_else(|| anyhow::anyhow!("archive table not readable"))?;
            Ok(rpf.read(entry)?)
        }
        Node::Gta4File(a, id) => Ok(a.read(id)?),
        _ => anyhow::bail!("{vpath} is not a file"),
    }
}

/// Reads a Red Dead Redemption 2 text table (`.yldb`) as text.
pub(crate) fn read_rdr2_text_table(vpath: &str) -> anyhow::Result<String> {
    let (game_dir, keys) = with_app(|a| (a.game_dir.clone(), a.keys.clone()));
    let game_dir = game_dir.ok_or_else(|| anyhow::anyhow!("no game folder"))?;
    match rage_mods::resolve(&game_dir, vpath, keys)?.0 {
        Node::Rpf8File(rpf, i) => Ok(openrage_app::texttable::rdr2_entry_to_text(&rpf, i)?.0),
        _ => anyhow::bail!("{vpath} is not a file in a Red Dead Redemption 2 archive"),
    }
}

/// Opens a text file: XML in the XML window (tree, fields, colored text),
/// anything else in the text editor.
pub(crate) fn open_text_file(ui: &AppWindow, vpath: String) {
    let lower = vpath.to_ascii_lowercase();
    if lower.ends_with(".ysc") || openrage_app::texttable::is_rdr2_text_table(&vpath) {
        return open_editor(ui, vpath);
    }
    ui.set_status(format!("Opening {vpath} …").into());
    let data = match read_file(ui, &vpath) {
        Ok(d) => d,
        Err(e) => return show_error(ui, format!("Cannot open {vpath}: {e:#}")),
    };
    if let Ok(xml) = openrage_app::metadata::from_bytes(&data)
        && xml.encoding == openrage_app::metadata::Encoding::Text
        && openrage_app::xmltree::parse(&xml.text).is_ok()
    {
        xmlview::open(ui, vpath, xml);
        ui.set_status(SharedString::new());
        return;
    }
    open_text_data(ui, vpath, &data);
}

pub(crate) fn open_text_data(ui: &AppWindow, vpath: String, data: &[u8]) {
    match editor::Document::from_file(vpath.clone(), data) {
        Ok(doc) => {
            editor::open(ui, doc);
            ui.set_status(SharedString::new());
        }
        Err(e) => show_error(ui, format!("Cannot open {vpath} as text ({e}); use Extract… instead.")),
    }
}

pub(crate) fn open_editor(ui: &AppWindow, vpath: String) {
    if vpath.to_ascii_lowercase().ends_with(".ysc") {
        return open_script_view(ui, vpath);
    }
    if openrage_app::texttable::is_rdr2_text_table(&vpath) {
        ui.set_status(format!("Reading {vpath} …").into());
        match read_rdr2_text_table(&vpath) {
            Ok(text) => {
                editor::open(ui, editor::Document::rdr2_text_table(vpath, text));
                ui.set_status(SharedString::new());
            }
            Err(e) => show_error(ui, format!("Cannot read {vpath}: {e:#}")),
        }
        return;
    }
    ui.set_status(format!("Opening {vpath} …").into());
    if vpath.to_ascii_lowercase().ends_with(".wpl") {
        // GTA IV placement file: as text, converted back on saving.
        let game = with_app(|a| a.game_dir.clone());
        let read = read_file(ui, &vpath)
            .and_then(|d| openrage_app::gta4::wpl_to_text(game.as_deref(), &d).map(|(text, _)| (text, d)));
        match read {
            Ok((text, data)) => {
                editor::open(ui, editor::Document::placement(vpath, text, data));
                ui.set_status(SharedString::new());
            }
            Err(e) => show_error(ui, format!("Cannot open {vpath}: {e:#}")),
        }
        return;
    }
    match read_file(ui, &vpath) {
        Ok(data) => open_text_data(ui, vpath, &data),
        Err(e) => show_error(ui, format!("Cannot open {vpath}: {e:#}")),
    }
}

/// Disassembles a script (`.ysc`).
pub(crate) fn read_script_listing(ui: &AppWindow, vpath: &str) -> anyhow::Result<String> {
    let data = read_file(ui, vpath)?;
    if vpath.to_ascii_lowercase().ends_with(".sco") {
        let name = vpath.rsplit('/').next().unwrap_or(vpath);
        return openrage_app::gta4::sco_listing(name, &data);
    }
    let res = rage_resource_from(&data)?;
    let s = rage_formats::ysc::parse(&res)?;
    let header = script_header(&s);
    Ok(header + &s.listing(openrage_app::natives::name).map_err(|e| anyhow::anyhow!(e))?)
}

pub(crate) fn rage_resource_from(data: &[u8]) -> anyhow::Result<rage_resource::Resource> {
    Ok(rage_resource::Resource::from_rsc7(data)?)
}

pub(crate) fn script_header(s: &rage_formats::ysc::Script) -> String {
    format!(
        "// {}: {} of code, {} natives, {} statics, {} strings\n// {}\n",
        s.name,
        human_size(s.code.len() as u64),
        s.natives.len(),
        s.statics.len(),
        s.all_strings().len(),
        if openrage_app::natives::count() > 0 {
            "Native calls show the native's name and hash; strings are noted next to STRING."
        } else {
            "Native calls show the native's hash (Help → Load native names… adds names); strings are noted next to STRING."
        }
    )
}

/// Shows a script's disassembly in a read-only editor window.
pub(crate) fn open_script_view(ui: &AppWindow, vpath: String) {
    ui.set_status(format!("Disassembling {vpath} …").into());
    match read_script_listing(ui, &vpath) {
        Ok(text) => {
            editor::open(ui, editor::Document::script(vpath, text));
            ui.set_status(SharedString::new());
        }
        Err(e) => show_error(ui, format!("Cannot disassemble {vpath}: {e:#}")),
    }
}

/// Reads a metadata file and converts it to XML.
pub(crate) fn read_xml(ui: &AppWindow, vpath: &str) -> anyhow::Result<openrage_app::metadata::Xml> {
    // RDR2 keeps much of its metadata as resources, which the game's own
    // resolution reads; GTA V files are read like any other file.
    if let Some((dir, keys)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone())))
        && is_rdr2_dir(&dir)
    {
        return openrage_app::metadata::load(&dir, keys, vpath);
    }
    openrage_app::metadata::from_bytes(&read_file(ui, vpath)?)
}

/// Shows a metadata file as XML in a read-only editor window.
pub(crate) fn open_xml_view(ui: &AppWindow, vpath: String) {
    ui.set_status(format!("Converting {vpath} to XML …").into());
    match read_xml(ui, &vpath) {
        Ok(xml) => {
            xmlview::open(ui, vpath, xml);
            ui.set_status(SharedString::new());
        }
        Err(e) => show_error(ui, format!("Cannot convert {vpath}: {e:#}")),
    }
}

/// Asks for a file name and writes a Scaleform movie as `.swf` for Flash tools.
pub(crate) fn export_swf(ui: &AppWindow, vpath: String) {
    let swf = match read_file(ui, &vpath).and_then(|d| Ok(rage_formats::media::gfx_to_swf(&d)?)) {
        Ok(s) => s,
        Err(e) => return show_error(ui, format!("Cannot convert {vpath}: {e:#}")),
    };
    let name = vpath.rsplit('/').next().unwrap_or(&vpath);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_owned();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export as SWF")
            .set_file_name(format!("{stem}.swf"))
            .add_filter("Flash movie", &["swf"])
            .save_file()
        else {
            return;
        };
        let msg = match std::fs::write(&path, &swf) {
            Ok(()) => format!("Exported {} (open it with a Flash tool such as JPEXS or Ruffle)", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Asks for a file name and writes a text table as text (`.oxt` for `.gxt2`, like OpenIV).
pub(crate) fn export_text_table(ui: &AppWindow, vpath: String) {
    use openrage_app::texttable::{self, Kind};
    let Some(kind) = Kind::of(&vpath) else { return };
    let text = match read_file(ui, &vpath).and_then(|d| texttable::to_text(kind, &d)) {
        Ok((t, _)) => t,
        Err(e) => return show_error(ui, format!("Cannot convert {vpath}: {e:#}")),
    };
    let name = vpath.rsplit('/').next().unwrap_or(&vpath);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_owned();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let ext = kind.text_extension();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export as text")
            .set_file_name(format!("{stem}.{ext}"))
            .add_filter(kind.name(), &[ext])
            .save_file()
        else {
            return;
        };
        let msg = match std::fs::write(&path, &text) {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Asks for a file name and writes a metadata file as XML (`name.ymap.xml`, like CodeWalker).
pub(crate) fn export_xml(ui: &AppWindow, vpath: String) {
    let xml = match read_xml(ui, &vpath) {
        Ok(x) => x,
        Err(e) => return show_error(ui, format!("Cannot convert {vpath}: {e:#}")),
    };
    let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_owned();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export XML")
            .set_file_name(format!("{name}.xml"))
            .add_filter("XML", &["xml"])
            .save_file()
        else {
            return;
        };
        let msg = match std::fs::write(&path, &xml.text) {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Saves a file into the mods folder (inside an archive or as a loose file),
/// recorded in the journal; calls `done` on the UI thread.
pub(crate) fn save_file(
    ui: &AppWindow,
    vpath: String,
    data: Vec<u8>,
    description: String,
    done: impl FnOnce(Result<String, String>) + Send + 'static,
) {
    let Some((game_dir, keys)) = with_app(|a| Some((a.game_dir.clone()?, a.keys.clone()))) else { return };
    let current = with_app(|a| a.listing.as_ref().map(|l| l.vpath.clone())).unwrap_or_default();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<String> {
            match install::split(&game_dir, &vpath) {
                Some(t) => {
                    let applied =
                        install::apply(&game_dir, keys, &t.archive, &[(t.inner, Op::Set(data))], &description)?;
                    Ok(if applied.created {
                        format!(
                            "Saved into {}/{} (archive copied there first)",
                            rage_mods::overlay::folder_name(&game_dir),
                            t.archive
                        )
                    } else {
                        format!("Saved into {}", applied.path.display())
                    })
                }
                None => {
                    let path = install::put_loose(&game_dir, &vpath, &data, &description)?;
                    Ok(format!("Saved {}", path.display()))
                }
            }
        })();
        let ide = vpath.to_ascii_lowercase().ends_with(".ide");
        if ide && result.is_ok() {
            // New or renamed models: read the definitions again.
            openrage_app::gta4::forget_texture_index(&game_dir);
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if result.is_ok() {
                after_mods_change(&ui, &current);
            }
            done(result.map_err(|e| format!("{e:#}")));
        });
    });
}

/// Opens a read-only copy of a file with the system's default application.
pub(crate) fn open_with_system(ui: &AppWindow, vpath: String) {
    let data = match read_file(ui, &vpath) {
        Ok(d) => d,
        Err(e) => {
            show_error(ui, format!("Cannot open {vpath}: {e:#}"));
            return;
        }
    };
    let name = vpath.rsplit('/').next().unwrap_or("file").to_owned();
    // Archive contents are untrusted: the system would run programs and
    // scripts instead of showing them.
    if openrage_app::plugins::is_executable(&name) {
        ui.set_status(format!("{name} could run as a program; export it instead to look at it").into());
        return;
    }
    let result = (|| -> std::io::Result<PathBuf> {
        let dir = std::env::temp_dir().join("openrage-open").join(format!("{:x}", rage_core::hash::joaat(&vpath)));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(vfs::safe_relative_path(&name).unwrap_or_else(|| "file".into()));
        if path.exists() {
            let mut perms = std::fs::metadata(&path)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms)?;
        }
        std::fs::write(&path, &data)?;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms)?;
        Ok(path)
    })();
    let path = match result {
        Ok(p) => p,
        Err(e) => {
            show_error(ui, format!("Cannot write a copy: {e}"));
            return;
        }
    };
    let launched = if cfg!(target_os = "windows") {
        // Not `cmd /C start`: cmd.exe would interpret `&`, `|` and `^` in the
        // file name (which comes from the archive) as commands.
        std::process::Command::new("explorer.exe").arg(&path).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&path).spawn()
    };
    let msg = match launched {
        Ok(_) => format!("Opened a read-only copy: {}", path.display()),
        Err(e) => format!("Cannot start the system app: {e}"),
    };
    ui.set_status(msg.into());
}
