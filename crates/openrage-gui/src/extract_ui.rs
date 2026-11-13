//! Extracting files to disk.

use crate::*;

pub(crate) fn start_extract(ui: &AppWindow) {
    if with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Textures { .. }))) {
        export_textures(ui, false);
        return;
    }
    if with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Audio { .. }))) {
        export_sounds(ui);
        return;
    }
    // GTA IV: the selected entry, or the whole folder (or an archive on disk).
    let gta4 = with_app(|a| {
        let listing = a.listing.as_ref()?;
        let item = a.selected.map(|s| &listing.items[s]);
        match (&listing.location, item) {
            (Location::Gta4(arc, dir), Some(item)) => {
                arc.find_child(*dir, &item.name).map(|i| Ok((Arc::clone(arc), i)))
            }
            (Location::Gta4(arc, dir), None) => Some(Ok((Arc::clone(arc), *dir))),
            (Location::Disk(dir), Some(item))
                if item.kind == ItemKind::Archive && rage_rpf::Gta4Archive::file_is_gta4(&dir.join(&item.name)) =>
            {
                Some(
                    rage_rpf::Gta4Archive::open_default(&dir.join(&item.name))
                        .map(|arc| (Arc::new(arc), rage_rpf::Gta4Archive::ROOT)),
                )
            }
            _ => None,
        }
    });
    match gta4 {
        Some(Ok((arc, id))) => {
            spawn_extract(ui, move |out, cancel, progress| {
                openrage_app::gta4::extract(&arc, id, out, cancel, progress)
            });
            return;
        }
        Some(Err(e)) => {
            ui.set_status(e.to_string().into());
            return;
        }
        None => {}
    }
    // Red Dead Redemption 2: the selected file or folder, or the whole folder.
    let rdr2 = with_app(|a| {
        let listing = a.listing.as_ref()?;
        let Location::Rpf8(rpf, dir) = &listing.location else { return None };
        let item = a.selected.map(|s| &listing.items[s]);
        Some(match item {
            Some(item) if item.kind == ItemKind::Dir => (Arc::clone(rpf), browser::join(dir, &item.name), None),
            Some(item) => (Arc::clone(rpf), dir.clone(), rpf.find(&browser::join(dir, &item.name))),
            None => (Arc::clone(rpf), dir.clone(), None),
        })
    });
    if let Some((rpf, dir, file)) = rdr2 {
        spawn_extract(ui, move |out, cancel, progress| {
            openrage_app::extract::extract_rpf8(&rpf, &dir, file, out, cancel, progress)
        });
        return;
    }
    let target = with_app(|a| -> Result<(Arc<Rpf7>, EntryId), String> {
        let listing = a.listing.as_ref().ok_or("nothing to extract")?;
        let item = a.selected.map(|s| &listing.items[s]);
        match (&listing.location, item) {
            (Location::Search { .. } | Location::Changes { .. }, Some(item)) if item.mark != Some(Mark::Removed) => {
                let dir = a.game_dir.clone().ok_or("no game directory")?;
                match vfs::resolve(&dir, item.vpath.as_deref().unwrap_or_default(), a.keys.clone()) {
                    Ok(Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id)) => Ok((rpf, id)),
                    Ok(_) => Err("Only archive contents can be extracted.".into()),
                    Err(e) => Err(e.to_string()),
                }
            }
            (Location::Archive(rpf, _), Some(item)) => Ok((Arc::clone(rpf), item.entry.expect("archive item"))),
            (Location::Archive(rpf, dir), None) => Ok((Arc::clone(rpf), *dir)),
            (Location::Disk(dir), Some(item)) if item.kind == ItemKind::Archive => {
                let rpf = Rpf7::open(&dir.join(&item.name), a.keys.clone()).map_err(|e| e.to_string())?;
                Ok((Arc::new(rpf), Rpf7::ROOT))
            }
            _ => Err("Select an archive or something inside an archive to extract.".into()),
        }
    });
    let (rpf, id) = match target {
        Ok(t) => t,
        Err(msg) => {
            ui.set_status(msg.into());
            return;
        }
    };
    let cancel = with_app(|a| {
        a.cancel.store(false, Ordering::Relaxed);
        Arc::clone(&a.cancel)
    });
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(out) = rfd::FileDialog::new().set_title("Extract to folder").pick_folder() else { return };
        let _ = weak.upgrade_in_event_loop(|ui| set_busy_cancellable(&ui, "Extracting…".into()));
        let mut last = Instant::now();
        let result = extract_entry(&rpf, id, &out, false, &cancel, &mut |s, path| {
            if last.elapsed() > Duration::from_millis(100) {
                last = Instant::now();
                let msg = format!("Extracting… {} files, {} — {path}", s.files, human_size(s.bytes));
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
            }
        });
        let msg = match result {
            Ok(s) if s.cancelled => format!("Cancelled after {} files.", s.files),
            Ok(s) => {
                let mut m = format!("Extracted {} files ({}) to {}", s.files, human_size(s.bytes), s.target.display());
                if !s.failures.is_empty() {
                    m.push_str(&format!(" — {} failures, first: {}", s.failures.len(), s.failures[0]));
                }
                m
            }
            Err(e) => format!("Extraction failed: {e:#}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| set_busy(&ui, false, msg));
    });
}

pub(crate) type ExtractProgress<'a> = &'a mut dyn FnMut(&openrage_app::extract::ExtractSummary, &str);

/// Asks for a target folder, then runs `job` in the background with progress
/// in the status bar; the cancel button stops it.
pub(crate) fn spawn_extract(
    ui: &AppWindow,
    job: impl FnOnce(&Path, &AtomicBool, ExtractProgress) -> anyhow::Result<openrage_app::extract::ExtractSummary>
    + Send
    + 'static,
) {
    let cancel = with_app(|a| {
        a.cancel.store(false, Ordering::Relaxed);
        Arc::clone(&a.cancel)
    });
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(out) = rfd::FileDialog::new().set_title("Extract to folder").pick_folder() else { return };
        let _ = weak.upgrade_in_event_loop(|ui| set_busy_cancellable(&ui, "Extracting…".into()));
        let mut last = Instant::now();
        let result = job(&out, &cancel, &mut |s, path| {
            if last.elapsed() > Duration::from_millis(100) {
                last = Instant::now();
                let msg = format!("Extracting… {} files, {} — {path}", s.files, human_size(s.bytes));
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
            }
        });
        let msg = match result {
            Ok(s) if s.cancelled => format!("Cancelled after {} files.", s.files),
            Ok(s) => {
                let mut m = format!("Extracted {} files ({}) to {}", s.files, human_size(s.bytes), out.display());
                if !s.failures.is_empty() {
                    m.push_str(&format!(" — {} could not be read", s.failures.len()));
                }
                m
            }
            Err(e) => format!("Extraction failed: {e:#}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| set_busy(&ui, false, msg));
    });
}
