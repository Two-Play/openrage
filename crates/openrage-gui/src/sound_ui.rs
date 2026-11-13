//! Sounds: the list of an audio container, playback and export.

use crate::*;

/// Selecting an `.awc`: its sounds as a list (click one to play it).
pub(crate) fn show_audio_container(ui: &AppWindow, vpath: String, mut info: Meta) {
    let (keys, rdr2) = with_app(|a| (a.keys.clone(), a.game_dir.as_deref().is_some_and(is_rdr2_dir)));
    let parse = |d: Vec<u8>| -> anyhow::Result<rage_formats::awc::Awc> {
        if rdr2 { Ok(rage_formats::awc::Awc::parse_rdr2(d)?) } else { openrage_app::audio::parse(d, keys.as_deref()) }
    };
    match read_file(ui, &vpath).and_then(parse) {
        Ok(awc) => {
            let (names, rows): (Vec<String>, Vec<ClipRow>) = match &awc.stream_format {
                Some(f) => {
                    let stem = vpath.rsplit('/').next().unwrap_or(&vpath).trim_end_matches(".awc").to_owned();
                    let secs = awc.sounds.first().map_or(0.0, |s| s.duration_secs());
                    let name = format!("{stem} (stream)");
                    let info = format!("{} · {} channels · music/stream", browser::seconds(secs), f.channels);
                    (vec![name.clone()], vec![ClipRow { name: name.into(), info: info.into() }])
                }
                None => awc
                    .sounds
                    .iter()
                    .map(|s| {
                        let name = format!("{:08x}", s.id);
                        let info = format!(
                            "{} · {} Hz · {}",
                            browser::seconds(s.duration_secs()),
                            s.sample_rate,
                            s.codec.name().replace("codec 7", "MPEG")
                        );
                        (name.clone(), ClipRow { name: name.into(), info: info.into() })
                    })
                    .unzip(),
            };
            let total: f32 = awc.sounds.iter().map(|s| s.duration_secs()).sum();
            info.push(kv("Contains", format!("{} sound(s)", rows.len())));
            if awc.stream_format.is_none() {
                info.push(kv("Total length", browser::seconds(total)));
            }
            if awc.encrypted || awc.flags & 0xA != 0 {
                info.push(kv("Encrypted", "yes"));
            }
            info.push(note("Click a sound to play it, or double-click the container to open it."));
            set_meta(ui, &info);
            ui.set_suggest_icon("audio".into());
            ui.set_suggest_title(format!("{} sound(s) in this container", rows.len()).into());
            ui.set_suggestions(ModelRc::new(VecModel::from(rows)));
            with_app(|a| {
                a.suggestions = names;
                a.suggest_awc = vpath;
            });
        }
        Err(e) => {
            info.push(kv("Error", format!("{e:#}")));
            set_meta(ui, &info);
        }
    }
}

pub(crate) fn start_sound_preview(
    ui: &AppWindow,
    awc: Arc<rage_formats::awc::Awc>,
    index: usize,
    info: Meta,
    generation: u64,
) {
    let counter = with_app(|a| Arc::clone(&a.preview_generation));
    ui.set_preview_loading(true);
    let (w, h) = (
        (ui.get_preview_area_width() - 16.0).max(64.0) as usize,
        (ui.get_preview_area_height() - 16.0).max(64.0) as usize,
    );
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let sound = awc.sounds.get(index).cloned();
        let result = if awc.stream_format.is_some() { awc.decode_stream() } else { awc.decode_sound(index) };
        let wave = result.as_ref().ok().map(|pcm| audio::waveform(pcm, w, h, [0x3b, 0x82, 0xf6]));
        if counter.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            ui.set_preview_loading(false);
            let mut meta = info.clone();
            match result {
                Ok(pcm) => {
                    meta.push(kv("Duration", browser::seconds(pcm.duration_secs())));
                    meta.push(kv("Sample rate", format!("{} Hz", pcm.sample_rate)));
                    meta.push(kv("Channels", pcm.channels));
                    if let Some(s) = &sound {
                        meta.push(kv("Codec", s.codec.name().replace("codec 7", "MPEG audio")));
                    }
                    if awc.encrypted {
                        meta.push(kv("Encrypted", "yes"));
                    }
                    set_meta(&ui, &meta);
                    if let Some(rgba) = wave {
                        let buffer =
                            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w as u32, h as u32);
                        ui.set_preview_image(slint::Image::from_rgba8(buffer));
                        ui.set_has_image(true);
                    }
                    let pcm = Arc::new(pcm);
                    with_app(|a| {
                        a.sound = Some(Arc::clone(&pcm));
                        a.sound_start = 0.0;
                    });
                    ui.set_audio_mode(true);
                    update_sound_neighbours(&ui);
                    update_sound_ui(&ui);
                    if with_app(|a| std::mem::take(&mut a.play_when_ready)) {
                        toggle_sound(&ui);
                    }
                }
                Err(e) => {
                    meta.push(kv("Error", e));
                    set_meta(&ui, &meta);
                }
            }
        });
    });
}

/// Selects the previous/next sound of the opened container (and plays it).
pub(crate) fn step_sound(ui: &AppWindow, delta: i32, play: bool) -> bool {
    let target = ui.get_file_selected() + delta;
    let is_sound = with_app(|a| {
        let l = a.listing.as_ref()?;
        let i = *a.visible.get(usize::try_from(target).ok()?)?;
        Some(l.items[i].kind == ItemKind::Sound)
    });
    if is_sound != Some(true) {
        return false;
    }
    with_app(|a| a.play_when_ready = play);
    ui.invoke_select_row(target);
    true
}

/// Whether there are sounds before/after the selected one.
pub(crate) fn update_sound_neighbours(ui: &AppWindow) {
    let row = ui.get_file_selected();
    let is_sound = |r: i32| {
        with_app(|a| {
            let l = a.listing.as_ref()?;
            let i = *a.visible.get(usize::try_from(r).ok()?)?;
            Some(l.items[i].kind == ItemKind::Sound)
        }) == Some(true)
    };
    ui.set_audio_has_prev(is_sound(row - 1));
    ui.set_audio_has_next(is_sound(row + 1));
}

/// How close to the end a position counts as "at the end": playing from
/// there starts the sound again from the beginning.
pub(crate) const SOUND_END_MARGIN: f32 = 0.05;

/// Play, pause or resume the selected sound.
pub(crate) fn toggle_sound(ui: &AppWindow) {
    let Some(pcm) = with_app(|a| a.sound.clone()) else { return };
    let total = pcm.duration_secs();
    let action = with_app(|a| match &a.output {
        Some(o) if Arc::ptr_eq(o.pcm(), &pcm) && !o.finished() => {
            if o.is_playing() {
                o.pause();
            } else {
                if o.position().as_secs_f32() >= total - SOUND_END_MARGIN {
                    o.seek(Duration::ZERO);
                }
                o.resume();
            }
            None
        }
        _ => Some(if a.sound_start >= total - SOUND_END_MARGIN { 0.0 } else { a.sound_start }),
    });
    if let Some(start) = action {
        match audio::Output::play(Arc::clone(&pcm), Duration::from_secs_f32(start)) {
            Ok(out) => with_app(|a| a.output = Some(out)),
            Err(e) => {
                ui.set_status(format!("Cannot play audio: {e}").into());
                return;
            }
        }
    }
    let weak = ui.as_weak();
    with_app(|a| {
        a.audio_timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
            if let Some(ui) = weak.upgrade() {
                update_sound_ui(&ui);
            }
        })
    });
    update_sound_ui(ui);
}

pub(crate) fn stop_sound(ui: &AppWindow) {
    with_app(|a| {
        a.output = None;
        a.sound_start = 0.0;
        a.audio_timer.stop();
    });
    update_sound_ui(ui);
}

/// Jumps to `fraction` (0..1) of the selected sound.
pub(crate) fn seek_sound(ui: &AppWindow, fraction: f32) {
    let Some(pcm) = with_app(|a| a.sound.clone()) else { return };
    let secs = pcm.duration_secs() * fraction.clamp(0.0, 1.0);
    with_app(|a| match &a.output {
        Some(o) if Arc::ptr_eq(o.pcm(), &pcm) && !o.finished() => o.seek(Duration::from_secs_f32(secs)),
        _ => a.sound_start = secs,
    });
    update_sound_ui(ui);
}

/// Position, time and play state of the player controls.
pub(crate) fn update_sound_ui(ui: &AppWindow) {
    let Some(pcm) = with_app(|a| a.sound.clone()) else { return };
    let total = pcm.duration_secs().max(0.001);
    let (pos, playing, finished) = with_app(|a| match &a.output {
        Some(o) if Arc::ptr_eq(o.pcm(), &pcm) => (o.position().as_secs_f32(), o.is_playing(), o.finished()),
        _ => (a.sound_start, false, false),
    });
    let pos = if finished { 0.0 } else { pos.min(total) };
    if finished {
        with_app(|a| {
            a.output = None;
            a.sound_start = 0.0;
            a.audio_timer.stop();
        });
        // "Play all": continue with the next sound (outside this timer tick).
        if ui.get_audio_play_all() {
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    step_sound(&ui, 1, true);
                }
            });
        }
    }
    ui.set_audio_position(pos / total);
    ui.set_audio_playing(playing && !finished);
    ui.set_audio_time(format!("{} / {}", browser::seconds(pos), browser::seconds(total)).into());
}

pub(crate) fn save_wav(ui: &AppWindow) {
    let Some(pcm) = with_app(|a| a.sound.clone()) else { return };
    let name = ui.get_info_title().to_string();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let stem = name.trim_end_matches(" (stream)").to_owned();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save WAV")
            .set_file_name(format!("{stem}.wav"))
            .add_filter("WAV audio", &["wav"])
            .save_file()
        else {
            return;
        };
        let msg = match std::fs::write(&path, pcm.to_wav()) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Saving failed: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Saves every sound of the opened container (or the selected one) as WAV.
pub(crate) fn export_sounds(ui: &AppWindow) {
    let Some((awc, selected, name)) = with_app(|a| {
        let l = a.listing.as_ref()?;
        let Location::Audio { awc } = &l.location else { return None };
        let name = l.vpath.rsplit('/').next().unwrap_or("audio").trim_end_matches(".awc").to_owned();
        Some((Arc::clone(awc), a.selected, name))
    }) else {
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(out) = rfd::FileDialog::new().set_title("Save WAV files to folder").pick_folder() else { return };
        let mut jobs: Vec<(String, Result<rage_formats::awc::Pcm, String>)> = Vec::new();
        if awc.stream_format.is_some() {
            jobs.push((format!("{name}.wav"), awc.decode_stream().map_err(|e| e.to_string())));
        } else {
            let picked: Vec<usize> = match selected {
                Some(i) => vec![i],
                None => (0..awc.sounds.len()).collect(),
            };
            for i in picked {
                let file = format!("{name}_{:08x}.wav", awc.sounds[i].id);
                jobs.push((file, awc.decode_sound(i).map_err(|e| e.to_string())));
            }
        }
        // Several sounds go into a subfolder named after the container, like OpenIV.
        let dir = if jobs.len() > 1 { out.join(&name) } else { out };
        let mut written = 0;
        let mut problems = Vec::new();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            problems.push(e.to_string());
        }
        for (file, pcm) in jobs {
            match pcm.and_then(|p| std::fs::write(dir.join(&file), p.to_wav()).map_err(|e| e.to_string())) {
                Ok(()) => written += 1,
                Err(e) => problems.push(format!("{file}: {e}")),
            }
        }
        let mut msg = format!("Saved {written} WAV file(s) to {}", dir.display());
        if let Some(first) = problems.first() {
            msg.push_str(&format!(" — {} problem(s), first: {first}", problems.len()));
        }
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}
