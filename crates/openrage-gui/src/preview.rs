//! Text / hex rendering of file contents for the preview pane.

use crate::*;
use std::fmt::Write;

const MAX_TEXT_BYTES: usize = 192 * 1024;
const MAX_TEXT_LINES: usize = 4000;
const MAX_HEX_BYTES: usize = 32 * 1024;

/// Renders `data` as text if it looks like text, otherwise as a hex dump.
pub fn render(data: &[u8]) -> String {
    match rage_core::text::decode_text(data, MAX_TEXT_BYTES) {
        Some(text) => truncate_text(text),
        None => hex_dump(data),
    }
}

fn truncate_text(text: String) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT_BYTES));
    for (i, line) in text.lines().enumerate() {
        if i == MAX_TEXT_LINES {
            out.push_str("\n… (preview truncated)");
            return out;
        }
        out.push_str(&line.replace('\t', "    "));
        out.push('\n');
    }
    if text.len() >= MAX_TEXT_BYTES {
        out.push_str("… (preview truncated)");
    }
    out
}

/// Classic 16-bytes-per-line hex dump with ASCII column.
pub fn hex_dump(data: &[u8]) -> String {
    let shown = &data[..data.len().min(MAX_HEX_BYTES)];
    let mut out = String::with_capacity(shown.len() * 4 + 64);
    for (line, chunk) in shown.chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", line * 16);
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => {
                    let _ = write!(out, "{b:02x} ");
                }
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        out.extend(chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }));
        out.push_str("|\n");
    }
    if data.len() > shown.len() {
        let _ = write!(out, "… ({} more bytes)", data.len() - shown.len());
    }
    out
}

/// What the preview worker needs to load an item off the UI thread.
pub(crate) enum PreviewSource {
    DiskFile(PathBuf),
    DiskArchive(PathBuf),
    Entry(Arc<Rpf7>, EntryId),
    /// A file in a Red Dead Redemption 2 archive (index into its entries).
    Rpf8(Arc<rage_rpf::Rpf8>, usize),
    /// A file in a GTA IV archive.
    Gta4(Arc<rage_rpf::Gta4Archive>, rage_rpf::Gta4EntryId),
    /// A texture of an opened texture dictionary (index into its textures).
    Texture(Arc<rage_formats::ytd::TextureDictionary>, usize),
    /// A search hit; resolved in the worker. `query` is set for text hits to show the matching line.
    Virtual {
        game_dir: PathBuf,
        vpath: String,
        query: Option<String>,
        /// Resolve through the mods overlay.
        overlay: bool,
    },
}

pub(crate) fn select_row(ui: &AppWindow, row: usize) {
    let Some((item, vpath, source, keys, generation)) = with_app(|a| {
        let listing = a.listing.as_ref()?;
        let idx = *a.visible.get(row)?;
        a.selected = Some(idx);
        let item = listing.items.get(idx)?.clone();
        let source = match (&listing.location, item.entry) {
            (Location::History, _) => None,
            (_, None) if item.mark == Some(Mark::Removed) => {
                // Removed by the mod: show the original.
                let vpath = item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name));
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual {
                    game_dir,
                    vpath,
                    query: None,
                    overlay: false,
                })
            }
            (Location::Changes { .. }, _) => {
                let vpath = item.vpath.clone().unwrap_or_default();
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual {
                    game_dir,
                    vpath,
                    query: None,
                    overlay: true,
                })
            }
            (Location::Search { query, .. }, _) => {
                let vpath = item.vpath.clone().unwrap_or_default();
                let query = (item.info == "text").then(|| match rage_index::parse_hash(query) {
                    Some(h) => format!("0x{h:08X}"),
                    None => query.clone(),
                });
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual { game_dir, vpath, query, overlay: false })
            }
            (Location::Textures { dict, .. }, _) => Some(PreviewSource::Texture(Arc::clone(dict), idx)),
            (Location::Disk(dir), _) if item.kind == ItemKind::Archive => {
                Some(PreviewSource::DiskArchive(dir.join(&item.name)))
            }
            (Location::Disk(dir), _) if item.kind == ItemKind::File => {
                Some(PreviewSource::DiskFile(dir.join(&item.name)))
            }
            (Location::Archive(rpf, _), Some(id)) if item.kind != ItemKind::Dir => {
                Some(PreviewSource::Entry(Arc::clone(rpf), id))
            }
            (Location::Rpf8(rpf, dir), _) if item.kind != ItemKind::Dir => {
                rpf.find(&browser::join(dir, &item.name)).map(|i| PreviewSource::Rpf8(Arc::clone(rpf), i))
            }
            (Location::Gta4(a, dir), _) if item.kind != ItemKind::Dir => {
                a.find_child(*dir, &item.name).map(|i| PreviewSource::Gta4(Arc::clone(a), i))
            }
            _ => None,
        };
        let generation = a.preview_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let vpath = item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name));
        Some((item, vpath, source, a.keys.clone(), generation))
    }) else {
        return;
    };
    ui.set_file_selected(row as i32);
    with_app(|a| a.pending_viewer = false);
    ui.set_image_viewable(is_viewable(&item));
    ui.set_can_extract(true);
    ui.set_texture_selected(item.kind == ItemKind::Texture);
    if with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::History))) {
        ui.set_history_group(item.info.clone().into());
    }
    ui.set_entry_selected(item.entry.is_some());

    let mut info = vec![kv("Type", badges::type_name(&item.name, item.kind)), kv("Size", human_size(item.size))];
    if !item.info.is_empty() && item.kind != ItemKind::Sound {
        // Resources show their version ("v171"); other items a short note.
        let is_version = item.info.strip_prefix('v').is_some_and(|v| v.chars().all(|c| c.is_ascii_digit()));
        info.push(kv(if is_version { "Version" } else { "Details" }, &item.info));
    }
    set_info(ui, &item.name, &vpath, &info, badges::icon(&item.name, item.kind));
    ui.set_preview_text(SharedString::new());
    ui.set_has_image(false);
    ui.set_model_mode(false);
    ui.set_model_textures(0);
    with_app(|a| a.model = None);
    stop_sound(ui);
    ui.set_audio_mode(false);
    with_app(|a| a.sound = None);
    ui.set_suggestions(ModelRc::default());
    ui.set_suggest_title(SharedString::new());
    ui.set_suggest_pending(false);
    with_app(|a| {
        a.suggestions.clear();
        a.suggest_ycd.clear();
        a.suggest_awc.clear();
        a.suggest_pending = false;
    });
    if is_clip_dictionary(&item) && item.mark != Some(Mark::Removed) {
        show_clip_dictionary(ui, vpath.clone(), info);
        return;
    }
    if browser::is_audio(&item.name) && item.kind == ItemKind::File && item.mark != Some(Mark::Removed) {
        show_audio_container(ui, vpath.clone(), info);
        return;
    }
    if item.kind == ItemKind::Sound {
        let awc = with_app(|a| match a.listing.as_ref().map(|l| &l.location) {
            Some(Location::Audio { awc }) => Some(Arc::clone(awc)),
            _ => None,
        });
        if let Some(awc) = awc {
            let index = with_app(|a| a.selected).unwrap_or(0);
            start_sound_preview(ui, awc, index, info, generation);
        }
        return;
    }
    if is_model_item(&item) {
        start_model_preview(ui, vpath.clone(), info, generation);
        return;
    }
    let Some(source) = source else {
        ui.set_preview_loading(false);
        return;
    };
    ui.set_preview_loading(true);

    let counter = with_app(|a| Arc::clone(&a.preview_generation));
    let weak = ui.as_weak();
    // Files the explorer has no viewer for may have one from a plugin.
    let plugin = (matches!(item.kind, ItemKind::File | ItemKind::Resource) && !badges::is_known(&item.name))
        .then(|| plugins::previews(&item.name))
        .flatten();
    let game = plugins::GameContext { dir: with_app(|a| a.game_dir.clone()), keys: keys.clone() };
    std::thread::spawn(move || {
        let mut from_plugin = plugin.and_then(|(id, specific)| plugin_preview(&id, specific, &game, &vpath));
        // A plugin's 3D model goes to the model view.
        if let Some(model) = from_plugin.as_mut().and_then(|p| p.model.take()) {
            let extra = from_plugin.map(|p| p.info).unwrap_or_default();
            let preview = model_preview(model);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if counter.load(Ordering::SeqCst) != generation {
                    return;
                }
                ui.set_preview_loading(false);
                show_model_preview(&ui, preview, info.iter().cloned().chain(extra).collect());
            });
            return;
        }
        let (extra, body, image) = match from_plugin {
            Some(p) => (p.info, p.text, p.image),
            None => load_preview(source, keys),
        };
        if counter.load(Ordering::SeqCst) != generation {
            return; // a newer selection superseded this one
        }
        let image =
            image.map(|(w, h, rgba)| slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h));
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            if !extra.is_empty() {
                set_meta(&ui, &info.iter().cloned().chain(extra).collect());
            }
            ui.set_preview_text(body.into());
            match image {
                Some(buffer) => {
                    ui.set_preview_image(slint::Image::from_rgba8(buffer));
                    ui.set_has_image(true);
                    if with_app(|a| std::mem::take(&mut a.pending_viewer)) {
                        open_viewer(&ui);
                    }
                }
                None => ui.set_has_image(false),
            }
            ui.set_preview_loading(false);
        });
    });
}

pub(crate) fn texture_info(t: &rage_formats::texture::Texture) -> Meta {
    vec![
        kv("Dimensions", format!("{} × {}", t.width, t.height)),
        kv("Format", t.format.name()),
        kv("Mip levels", t.levels),
        kv("Usage", t.usage_name()),
        kv("Data", human_size(t.data.len() as u64)),
    ]
}

/// Textures shown in a dictionary's overview.
pub(crate) const SHEET_CELLS: usize = 16;

/// An overview of a texture dictionary: up to 16 textures in a grid
/// (aspect kept, transparent background).
pub(crate) fn texture_sheet(textures: &[rage_formats::texture::Texture]) -> Option<Rgba> {
    let picked: Vec<Rgba> = textures.iter().take(SHEET_CELLS).filter_map(texture_image).collect();
    if picked.is_empty() {
        return None;
    }
    if picked.len() == 1 {
        return picked.into_iter().next();
    }
    let columns = (picked.len() as f32).sqrt().ceil() as usize;
    let rows = picked.len().div_ceil(columns);
    const CELL: usize = 192;
    const GAP: usize = 8;
    let (w, h) = (columns * CELL + (columns - 1) * GAP, rows * CELL + (rows - 1) * GAP);
    let mut out = vec![0u8; w * h * 4];
    for (i, (tw, th, rgba)) in picked.iter().enumerate() {
        let (tw, th) = (*tw as usize, *th as usize);
        let scale = (CELL as f32 / tw.max(th) as f32).min(1.0);
        let (sw, sh) = (((tw as f32 * scale) as usize).max(1), ((th as f32 * scale) as usize).max(1));
        let (cx, cy) = ((i % columns) * (CELL + GAP) + (CELL - sw) / 2, (i / columns) * (CELL + GAP) + (CELL - sh) / 2);
        for y in 0..sh {
            for x in 0..sw {
                let (sx, sy) = (x * tw / sw, y * th / sh);
                let src = (sy * tw + sx) * 4;
                let dst = ((cy + y) * w + cx + x) * 4;
                out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
            }
        }
    }
    Some((w as u32, h as u32, out))
}

/// Summary and texture sheet of a texture dictionary for the preview.
pub(crate) fn texture_dict_preview(dict: &rage_formats::ytd::TextureDictionary) -> (Meta, String, Option<Rgba>) {
    let names: Vec<&str> = dict.textures.iter().take(8).map(|t| t.name.as_str()).collect();
    let more = if dict.textures.len() > 8 { ", …" } else { "" };
    let shown = dict.textures.len().min(SHEET_CELLS);
    let meta = vec![
        kv("Textures", dict.textures.len()),
        kv("Contains", format!("{}{more}", names.join(", "))),
        note(format!("The preview shows {shown} of them. Double-click to open the dictionary.")),
    ];
    (meta, String::new(), texture_sheet(&dict.textures))
}

pub(crate) fn texture_image(t: &rage_formats::texture::Texture) -> Option<Rgba> {
    t.to_rgba().map(|rgba| (t.width, t.height, rgba))
}

/// Preview with an optional image: textures and texture dictionaries show a picture.
pub(crate) fn load_preview(source: PreviewSource, keys: Option<Arc<GtaVKeys>>) -> (Meta, String, Option<Rgba>) {
    match source {
        PreviewSource::Texture(dict, idx) => match dict.textures.get(idx) {
            Some(t) => {
                let image = texture_image(t);
                let mut meta = texture_info(t);
                if image.is_none() {
                    meta.push(note("No preview decoder for this format (DDS export works)."));
                }
                (meta, String::new(), image)
            }
            None => (Meta::new(), String::new(), None),
        },
        PreviewSource::Gta4(a, id) if browser::is_texture_dict(&a.entry(id).name) => {
            match a
                .read_resource(id)
                .map_err(|e| e.to_string())
                .and_then(|r| rage_formats::ytd::parse_gta4(&r).map_err(|e| e.to_string()))
            {
                Ok(dict) => texture_dict_preview(&dict),
                Err(e) => (vec![kv("Error", e)], String::new(), None),
            }
        }
        PreviewSource::Entry(rpf, id) if browser::is_texture_dict(&rpf.entry(id).name) => {
            match rpf
                .read_resource(id)
                .map_err(|e| e.to_string())
                .and_then(|r| rage_formats::ytd::parse(&r).map_err(|e| e.to_string()))
            {
                Ok(dict) => texture_dict_preview(&dict),
                Err(e) => (vec![kv("Error", e)], String::new(), None),
            }
        }
        PreviewSource::Entry(rpf, id) if rpf.entry(id).name.to_ascii_lowercase().ends_with(".bik") => {
            let name = rpf.entry(id).name.clone();
            let (meta, body) = load_preview_text(PreviewSource::Entry(Arc::clone(&rpf), id), keys);
            let frame = rpf.read(id).ok().and_then(|d| video_frame(&name, &d));
            (meta, body, frame)
        }
        other => {
            let (meta, body) = load_preview_text(other, keys);
            (meta, body, None)
        }
    }
}

/// A frame from the middle of a Bink video, if FFmpeg is installed (it
/// decodes Bink; OpenRAGE has no decoder of its own).
pub(crate) fn video_frame(name: &str, data: &[u8]) -> Option<Rgba> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let b = rage_formats::media::bink(data).ok()?;
    if b.width == 0 || b.height == 0 {
        return None;
    }
    let (w, h) = (640u32.min(b.width), (640u32.min(b.width) * b.height / b.width).max(1) & !1);
    let dir = std::env::temp_dir().join("openrage-preview");
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("{:08x}.bik", rage_core::hash::joaat(name)));
    std::fs::File::create(&file).ok()?.write_all(data).ok()?;
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-ss", &format!("{:.2}", b.duration() / 2.0), "-i"])
        .arg(&file)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok();
    let _ = std::fs::remove_file(&file);
    let pixels = output?.stdout;
    (pixels.len() == (w * h * 4) as usize).then_some((w, h, pixels))
}

/// Returns (metadata, preview body).
pub(crate) fn load_preview_text(source: PreviewSource, keys: Option<Arc<GtaVKeys>>) -> (Meta, String) {
    match source {
        PreviewSource::Virtual { game_dir, vpath, query, overlay } => match if overlay {
            rage_mods::resolve(&game_dir, &vpath, keys.clone()).map(|(n, _)| n)
        } else {
            vfs::resolve(&game_dir, &vpath, keys.clone())
        } {
            Ok(Node::ArchiveFile(rpf, id)) => {
                let (mut meta, body) = load_preview_text(PreviewSource::Entry(Arc::clone(&rpf), id), keys);
                if let Some(q) = query {
                    let name = rpf.entry(id).name.clone();
                    let line = rpf
                        .read(id)
                        .ok()
                        .and_then(|d| rage_index::content_for(&name, &d))
                        .and_then(|(text, _)| rage_index::snippet(&text, &q));
                    if let Some(line) = line {
                        meta.insert(0, kv("Match", line));
                    }
                }
                (meta, body)
            }
            Ok(Node::ArchiveDir(rpf, id)) if id == Rpf7::ROOT => (archive_info(&rpf), String::new()),
            Ok(Node::ArchiveDir(..)) | Ok(Node::Dir(_)) => (Meta::new(), String::new()),
            Ok(Node::PlainFile(p)) => load_preview_text(PreviewSource::DiskFile(p), keys),
            Ok(Node::Rpf8File(rpf, i)) => rpf8_preview(&rpf, i),
            Ok(Node::Rpf8Dir(..)) => (Meta::new(), String::new()),
            Ok(Node::Gta4File(a, i)) => gta4_preview(&a, i),
            Ok(Node::Gta4Dir(a, i)) if i == rage_rpf::Gta4Archive::ROOT => (gta4_archive_info(&a), String::new()),
            Ok(Node::Gta4Dir(..)) => (Meta::new(), String::new()),
            Ok(_) => (Meta::new(), String::new()),
            Err(e) => (vec![kv("Error", e)], String::new()),
        },
        PreviewSource::Rpf8(rpf, i) => rpf8_preview(&rpf, i),
        PreviewSource::Gta4(a, i) => gta4_preview(&a, i),
        PreviewSource::Texture(..) => (Meta::new(), String::new()),
        PreviewSource::DiskFile(path)
            if openrage_app::metadata::is_metadata(&path.file_name().unwrap_or_default().to_string_lossy()) =>
        {
            match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|d| openrage_app::metadata::from_bytes(&d))
            {
                Ok(xml) => metadata_lines(xml),
                Err(e) => (vec![kv("Error", e)], String::new()),
            }
        }
        PreviewSource::DiskFile(path) if openrage_app::texttable::Kind::of(&path.to_string_lossy()).is_some() => {
            let kind = openrage_app::texttable::Kind::of(&path.to_string_lossy()).expect("checked");
            match std::fs::read(&path) {
                Ok(data) => text_table_preview(kind, &data),
                Err(e) => (vec![kv("Error", e)], String::new()),
            }
        }
        PreviewSource::DiskFile(path) if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wpl")) => {
            let game = openrage_app::gta4::game_folder(&path);
            match std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .and_then(|d| openrage_app::gta4::wpl_to_text(game.as_deref(), &d))
            {
                Ok(converted) => wpl_preview(table_text_preview("GTA IV placement file", Ok(converted))),
                Err(e) => (vec![kv("Error", format!("{e:#}"))], String::new()),
            }
        }
        PreviewSource::DiskFile(path) => {
            let mut data = Vec::new();
            match File::open(&path).and_then(|f| f.take(1 << 20).read_to_end(&mut data)) {
                Ok(_) => (Meta::new(), preview::render(&data)),
                Err(e) => (vec![kv("Error", e)], String::new()),
            }
        }
        PreviewSource::DiskArchive(path) if rage_rpf::Gta4Archive::file_is_gta4(&path) => {
            match rage_rpf::Gta4Archive::open_default(&path) {
                Ok(a) => (gta4_archive_info(&a), String::new()),
                Err(e) => (vec![kv("Error", e)], String::new()),
            }
        }
        PreviewSource::DiskArchive(path) => match Rpf7::open(&path, keys) {
            Ok(rpf) => (archive_info(&rpf), String::new()),
            Err(e) => (vec![kv("Error", e)], String::new()),
        },
        PreviewSource::Entry(rpf, id) => {
            let entry = rpf.entry(id).clone();
            if entry.is_archive() {
                return match rpf.open_child(id) {
                    Ok(child) => (archive_info(&child), String::new()),
                    Err(e) => (vec![kv("Error", e)], String::new()),
                };
            }
            if openrage_app::metadata::is_metadata(&entry.name)
                && let Some(preview) = metadata_preview(&rpf, id)
            {
                return preview;
            }
            if openrage_app::media::is_media(&entry.name)
                && let Ok(data) = rpf.read(id)
            {
                return match openrage_app::media::rows(&entry.name, &data) {
                    Ok(rows) => {
                        let mut meta: Meta = rows.into_iter().filter(|(_, v)| !v.is_empty()).collect();
                        meta.push(note(if entry.name.to_ascii_lowercase().ends_with(".bik") {
                            "Double-click plays it in your video player (VLC, mpv and other FFmpeg-based players play Bink)."
                        } else {
                            "Right-click → Export as SWF… opens it in Flash tools such as JPEXS or Ruffle."
                        }));
                        (meta, String::new())
                    }
                    Err(e) => (vec![kv("Error", format!("{e:#}"))], String::new()),
                };
            }
            if entry.name.to_ascii_lowercase().ends_with(".ysc")
                && let Some(preview) = script_preview(&rpf, id)
            {
                return preview;
            }
            if let Some(kind) = openrage_app::texttable::Kind::of(&entry.name)
                && let Ok(data) = rpf.read(id)
            {
                return text_table_preview(kind, &data);
            }
            match entry.kind {
                EntryKind::Resource { size, .. } => {
                    let (sys, gfx) = entry.resource_page_sizes().unwrap_or_default();
                    let mut meta = vec![
                        kv("Resource version", entry.resource_version().unwrap_or_default()),
                        kv("Stored", human_size(size as u64)),
                        kv("System pages", human_size(sys)),
                        kv("Graphics pages", human_size(gfx)),
                    ];
                    match rpf.read_resource_pages(id) {
                        Ok(pages) => (meta, preview::hex_dump(&pages)),
                        Err(e) => {
                            meta.push(kv("Error", e));
                            (meta, String::new())
                        }
                    }
                }
                EntryKind::Binary { stored_size, size, encrypted, .. } => {
                    let mut meta = Meta::new();
                    if stored_size > 0 {
                        meta.push(kv(
                            "Compressed",
                            format!("{} → {}", human_size(stored_size as u64), human_size(size as u64)),
                        ));
                    }
                    if encrypted {
                        meta.push(kv("Encrypted", "yes"));
                    }
                    match rpf.read(id) {
                        Ok(data) => (meta, preview::render(&data)),
                        Err(e) => (vec![kv("Error", e)], String::new()),
                    }
                }
                EntryKind::Directory { .. } => (Meta::new(), String::new()),
            }
        }
    }
}

/// A text table shown as text in the preview.
pub(crate) fn text_table_preview(kind: openrage_app::texttable::Kind, data: &[u8]) -> (Meta, String) {
    table_text_preview(kind.name(), openrage_app::texttable::to_text(kind, data))
}

/// A placement file's preview: double-click shows it in 3D (not in the editor).
pub(crate) fn wpl_preview((meta, text): (Meta, String)) -> (Meta, String) {
    let mut meta: Meta = meta.into_iter().filter(|(k, v)| !(k.is_empty() && v.contains("ouble-click"))).collect();
    meta.push(note("Double-click to see its objects in 3D."));
    (meta, text)
}

/// A text table already converted to text (with its entry count) for the preview.
pub(crate) fn table_text_preview(format: &str, converted: anyhow::Result<(String, usize)>) -> (Meta, String) {
    const LIMIT: usize = 256 * 1024;
    match converted {
        Ok((text, count)) => {
            // The preview font has no tab glyph.
            let mut text = text.replace('\t', "    ");
            let mut meta = vec![kv("Format", format), kv("Entries", count)];
            if text.len() > LIMIT {
                let cut = (0..=LIMIT).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
                text.truncate(cut);
                text.push_str("\n…\n");
                meta.push(note("Showing the start; double-click to open all of it in the editor."));
            } else {
                meta.push(note("Double-click to open it in the editor."));
            }
            (meta, text)
        }
        Err(e) => (vec![kv("Error", format!("{e:#}"))], String::new()),
    }
}

/// Metadata shown as XML in the preview (the start of it for big files).
pub(crate) fn metadata_preview(rpf: &Rpf7, id: EntryId) -> Option<(Meta, String)> {
    let xml = rpf.read(id).ok().and_then(|d| openrage_app::metadata::from_bytes(&d).ok())?;
    Some(metadata_lines(xml))
}

pub(crate) fn metadata_lines(xml: openrage_app::metadata::Xml) -> (Meta, String) {
    const LIMIT: usize = 256 * 1024;
    let mut meta = vec![kv("Format", xml.encoding.name())];
    if let Some(root) = &xml.root {
        meta.push(kv("Content", root));
    }
    meta.push(kv("XML", human_size(xml.text.len() as u64)));
    if !xml.errors.is_empty() {
        meta.push(kv("Unreadable parts", xml.errors.len()));
    }
    let mut text = xml.text;
    if text.len() > LIMIT {
        let cut = (0..=LIMIT).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        text.truncate(cut);
        text.push_str("\n…\n");
        meta.push(note("Showing the start; double-click to see all of it."));
    } else {
        meta.push(note("Double-click to open it as XML."));
    }
    (meta, text)
}

/// Facts about a file in an RDR2 archive, and its text when it is text.
pub(crate) fn rpf8_preview(rpf: &rage_rpf::Rpf8, i: usize) -> (Meta, String) {
    use rage_rpf::rpf8::Compression;
    let Some(e) = rpf.entries().and_then(|es| es.get(i)) else { return (Meta::new(), String::new()) };
    // Type and size are already in the file's general info.
    let mut meta = Meta::new();
    meta.push(kv("Stored", openrage_app::extract::human_size(e.size_on_disk)));
    meta.push(kv(
        "Compression",
        match e.compression {
            Compression::None => "none".to_owned(),
            Compression::Deflate => "deflate".to_owned(),
            Compression::Oodle => "Oodle".to_owned(),
            Compression::Unknown(n) => format!("unknown ({n})"),
        },
    ));
    meta.push(kv("Encrypted", if e.is_encrypted() { format!("yes (key {})", e.key_id) } else { "no".to_owned() }));
    if let Some(v) = e.resource_version() {
        meta.push(kv("Resource version", v));
    }
    meta.push(kv("Hash", format!("0x{:08X}", e.hash)));
    if e.is_resource && openrage_app::texttable::is_rdr2_text_table(&rpf.entry_name(i)) {
        let (table, text) = table_text_preview("RDR2 text table", openrage_app::texttable::rdr2_entry_to_text(rpf, i));
        meta.extend(table);
        return (meta, text);
    }
    if e.ext_id != 0 && openrage_app::metadata::is_metadata(&rpf.entry_name(i)) && e.size() <= 64 << 20 {
        return match openrage_app::metadata::from_rpf8(rpf, i) {
            Ok(xml) => {
                let (more, text) = metadata_lines(xml);
                meta.extend(more);
                (meta, text)
            }
            Err(err) => {
                meta.push(kv("Error", format!("{err:#}")));
                (meta, String::new())
            }
        };
    }
    if e.ext_id == 0 || e.is_resource || e.size() > 4 << 20 {
        return (meta, String::new());
    }
    match rpf.read(e) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data[..data.len().min(256 << 10)]).replace('\t', "    ");
            let printable = text.chars().filter(|c| !c.is_control() || c.is_whitespace()).count();
            if printable * 100 >= text.chars().count().max(1) * 95 { (meta, text) } else { (meta, String::new()) }
        }
        Err(err) => {
            meta.push(kv("Error", err));
            (meta, String::new())
        }
    }
}

/// Format, encryption and counts of a GTA IV archive.
pub(crate) fn gta4_archive_info(a: &rage_rpf::Gta4Archive) -> Meta {
    let ids = 1..a.len();
    let dirs = ids.clone().filter(|&i| a.entry(i).is_dir()).count();
    let resources = ids.clone().filter(|&i| a.entry(i).resource_type().is_some()).count();
    let mut meta = vec![
        kv("Archive", a.display_name()),
        kv("Format", format!("{} (GTA IV)", a.format().name())),
        kv("Encryption", if a.is_encrypted() { "AES" } else { "none" }),
    ];
    if dirs > 0 {
        meta.push(kv("Folders", dirs));
    }
    meta.push(kv("Files", a.len() - 1 - dirs - resources));
    meta.push(kv("Resources", resources));
    meta
}

/// Details of a file in a GTA IV archive; plain text files are shown as text.
pub(crate) fn gta4_preview(a: &rage_rpf::Gta4Archive, i: rage_rpf::Gta4EntryId) -> (Meta, String) {
    let e = a.entry(i);
    let mut meta = Meta::new();
    match e.kind {
        rage_rpf::Gta4EntryKind::Resource { resource_type, .. } => {
            let (sys, gfx) = e.resource_sizes().unwrap_or_default();
            meta.push(kv(
                "Resource",
                format!("RSC5 type {resource_type} ({})", openrage_app::gta4::resource_type_name(resource_type)),
            ));
            meta.push(kv("System memory", human_size(sys as u64)));
            meta.push(kv("Graphics memory", human_size(gfx as u64)));
            match a.read_resource(i) {
                Ok(res) => {
                    let mut pages = res.system;
                    pages.truncate(64 * 1024);
                    (meta, preview::hex_dump(&pages))
                }
                Err(err) => {
                    meta.push(kv("Error", err));
                    (meta, String::new())
                }
            }
        }
        rage_rpf::Gta4EntryKind::File { .. } if e.name.to_ascii_lowercase().ends_with(".wpl") => {
            let game = openrage_app::gta4::game_folder(a.path());
            let converted = a
                .read(i)
                .map_err(anyhow::Error::from)
                .and_then(|d| openrage_app::gta4::wpl_to_text(game.as_deref(), &d));
            let (more, text) = wpl_preview(table_text_preview("GTA IV placement file", converted));
            meta.extend(more);
            (meta, text)
        }
        rage_rpf::Gta4EntryKind::File { .. } if e.name.to_ascii_lowercase().ends_with(".sco") => {
            let data = a.read(i).ok();
            match data.as_deref().and_then(openrage_app::gta4::sco_rows) {
                Some(rows) => {
                    meta.extend(rows.into_iter().map(|(k, v)| kv(&k, v)));
                    meta.push(note("Double-click to read the disassembly."));
                }
                None => meta.push(kv("Error", "not a GTA IV script")),
            }
            // The strings the script uses, like for GTA V scripts.
            let strings =
                data.and_then(|d| openrage_app::gta4::read_sco(&d).ok()).map(|s| s.strings()).unwrap_or_default();
            let mut text = String::new();
            if !strings.is_empty() {
                text.push_str("Strings used by the script:\n\n");
                for st in strings.iter().take(400) {
                    text.push_str(st);
                    text.push('\n');
                }
                if strings.len() > 400 {
                    text.push_str(&format!("… ({} in all)\n", strings.len()));
                }
            }
            (meta, text)
        }
        rage_rpf::Gta4EntryKind::File { stored, compressed, .. } => {
            if compressed {
                meta.push(kv("Compressed", format!("{} → {}", human_size(stored), human_size(e.size()))));
            }
            if e.size() > 4 << 20 {
                return (meta, String::new());
            }
            match a.read(i) {
                Ok(data) => (meta, preview::render(&data)),
                Err(err) => (vec![kv("Error", err)], String::new()),
            }
        }
        rage_rpf::Gta4EntryKind::Dir { .. } => (meta, String::new()),
    }
}

pub(crate) fn archive_info(rpf: &Rpf7) -> Meta {
    let (mut dirs, mut files, mut resources, mut archives) = (0, 0, 0, 0);
    for e in rpf.entries() {
        match e.kind_label() {
            "dir" => dirs += 1,
            "rpf" => archives += 1,
            "resource" => resources += 1,
            _ => files += 1,
        }
    }
    vec![
        kv("Format", "RPF7"),
        kv("Encryption", format!("{:?}", rpf.encryption())),
        kv("Folders", dirs),
        kv("Files", files),
        kv("Resources", resources),
        kv("Nested archives", archives),
    ]
}

/// A script's facts and the start of its strings, for the preview.
pub(crate) fn script_preview(rpf: &Rpf7, id: EntryId) -> Option<(Meta, String)> {
    let s = rage_formats::ysc::parse(&rpf.read_resource(id).ok()?).ok()?;
    let functions = s.disassemble().map(|c| c.iter().filter(|i| i.name == "ENTER").count()).unwrap_or(0);
    let strings = s.all_strings();
    let meta = vec![
        kv("Script", &s.name),
        kv("Code", human_size(s.code.len() as u64)),
        kv("Functions", functions),
        kv("Natives", s.natives.len()),
        kv("Statics", s.statics.len()),
        kv("Strings", strings.len()),
        note("Double-click to read the disassembly."),
    ];
    let mut text = String::from("Strings used by the script:\n\n");
    for st in strings.iter().take(400) {
        text.push_str(st);
        text.push('\n');
    }
    if strings.len() > 400 {
        text.push_str(&format!("… ({} in all)\n", strings.len()));
    }
    Some((meta, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_text_and_binary() {
        assert!(render(b"<?xml version=\"1.0\"?>\n<a/>").starts_with("<?xml"));
        assert!(render(&[0u8, 1, 2, 3]).starts_with("00000000  00 01 02 03"));
        let utf16: Vec<u8> =
            [0xFF, 0xFE].into_iter().chain("hi".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert!(render(&utf16).starts_with("hi"));
    }
}
