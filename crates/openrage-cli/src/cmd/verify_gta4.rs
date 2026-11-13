//! `openrage verify` for GTA IV.

use crate::*;

/// Reads every entry of every GTA IV archive below `within` (or of the one
/// archive it names) and inflates every resource.
pub(crate) type Gta4Start = (String, Arc<rage_rpf::Gta4Archive>, rage_rpf::Gta4EntryId);

/// The GTA IV archives to process: the folder or archive `within` names, or
/// every archive below a real folder. Archives that fail to open are returned
/// as errors.
pub(crate) fn gta4_starts(game_dir: &Path, within: &str) -> Result<(Vec<Gta4Start>, Vec<String>)> {
    use rage_rpf::Gta4Archive;
    let mut open_errors = Vec::new();
    let starts: Vec<Gta4Start> = match vfs::resolve(game_dir, within, None)? {
        Node::Gta4Dir(a, id) => vec![(within.trim_end_matches('/').to_owned(), a, id)],
        Node::Dir(dir) => {
            let mut found = Vec::new();
            collect_archives(&dir, &["rpf", "img"], &mut found);
            found
                .into_iter()
                .filter(|p| Gta4Archive::file_is_gta4(p))
                .filter_map(|p| {
                    let rel = p.strip_prefix(game_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    match Gta4Archive::open_default(&p) {
                        Ok(a) => Some((rel, Arc::new(a), Gta4Archive::ROOT)),
                        Err(e) => {
                            open_errors.push(format!("{rel}: {e}"));
                            None
                        }
                    }
                })
                .collect()
        }
        _ => bail!("{within} is not a folder or archive"),
    };
    Ok((starts, open_errors))
}

pub(crate) fn cmd_verify_gta4(game_dir: &Path, within: &str) -> Result<()> {
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    // Audio entries (RPF3) with a name instead of only a hash.
    let audio = std::sync::atomic::AtomicUsize::new(0);
    let audio_named = std::sync::atomic::AtomicUsize::new(0);
    let stats: Vec<VerifyStats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = VerifyStats { archives: 1, ..Default::default() };
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                if let Some(hash) = a.audio_hash_of(id) {
                    use std::sync::atomic::Ordering::Relaxed;
                    audio.fetch_add(1, Relaxed);
                    if e.name != format!("{hash:08x}") {
                        audio_named.fetch_add(1, Relaxed);
                    }
                }
                let result = if e.is_dir() {
                    return;
                } else if e.resource_type().is_some() {
                    s.resources += 1;
                    a.read_resource(id).map(|r| r.system.len() + r.graphics.len())
                } else {
                    s.files += 1;
                    a.read(id).map(|d| d.len())
                };
                match result {
                    Ok(n) => s.bytes += n as u64,
                    Err(err) => s.errors.push(format!("{p}: {err}")),
                }
            });
            eprintln!("  checked {prefix}");
            s
        })
        .collect();
    let mut total = VerifyStats { archives: open_errors.len(), errors: open_errors, ..Default::default() };
    for s in stats {
        total.archives += s.archives;
        total.files += s.files;
        total.resources += s.resources;
        total.bytes += s.bytes;
        total.errors.extend(s.errors);
    }
    for e in &total.errors {
        outln!("FAIL {e}");
    }
    let audio = audio.into_inner();
    if audio > 0 {
        outln!(
            "{} of {audio} audio entries named (from the game's audio configuration); the others show their hash",
            audio_named.into_inner()
        );
    }
    outln!(
        "{} archives, {} files, {} resources, {} decoded, {} errors ({:.1}s)",
        total.archives,
        total.files,
        total.resources,
        human(total.bytes),
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("verification found {} errors", total.errors.len());
    }
    Ok(())
}

/// Disassembles every GTA IV script (`.sco`) and checks every jump, call and switch case.
pub(crate) fn cmd_verify_gta4_scripts(game_dir: &Path, within: &str) -> Result<()> {
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<(usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let (mut files, mut instructions, mut errors) = (0, 0, Vec::new());
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                if !a.entry(id).name.to_ascii_lowercase().ends_with(".sco") {
                    return;
                }
                files += 1;
                let checked = a
                    .read(id)
                    .map_err(anyhow::Error::from)
                    .and_then(|d| openrage_app::gta4::read_sco(&d))
                    .and_then(|s| s.check().map_err(|e| anyhow::anyhow!(e)));
                match checked {
                    Ok(n) => instructions += n,
                    Err(e) => errors.push(format!("{p}: {e:#}")),
                }
            });
            (files, instructions, errors)
        })
        .collect();
    let (mut files, mut instructions, mut errors) = (0, 0, open_errors);
    for (f, i, e) in all {
        files += f;
        instructions += i;
        errors.extend(e);
    }
    for e in errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "{files} scripts, {instructions} instructions, {} errors ({:.1}s)",
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

/// Reads every GTA IV animation dictionary (`.wad`) and evaluates every frame of every track.
pub(crate) fn cmd_verify_gta4_animations(game_dir: &Path, within: &str) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        files: usize,
        animations: usize,
        tracks: usize,
        frames: usize,
        bad_values: usize,
        errors: Vec<String>,
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = Stats::default();
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                if e.resource_type().is_none() || !e.name.to_ascii_lowercase().ends_with(".wad") {
                    return;
                }
                s.files += 1;
                match a
                    .read_resource(id)
                    .map_err(anyhow::Error::from)
                    .and_then(|r| Ok(rage_formats::wad::parse_wad(&r)?))
                {
                    Ok(dict) => {
                        for anim in &dict.animations {
                            s.animations += 1;
                            s.tracks += anim.tracks.len();
                            s.frames += anim.frames;
                            for t in 0..anim.tracks.len() {
                                for f in 0..anim.frames {
                                    let ok = anim.value(t, f as f32).is_some_and(|v| v.iter().all(|c| c.is_finite()));
                                    if !ok {
                                        s.bad_values += 1;
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => s.errors.push(format!("{p}: {err:#}")),
                }
            });
            s
        })
        .collect();
    let mut total = Stats { errors: open_errors, ..Default::default() };
    for s in all {
        total.files += s.files;
        total.animations += s.animations;
        total.tracks += s.tracks;
        total.frames += s.frames;
        total.bad_values += s.bad_values;
        total.errors.extend(s.errors);
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} animation dictionaries, {} animations ({} frames), {} bone tracks, {} values not finite, {} errors ({:.1}s)",
        total.files,
        total.animations,
        total.frames,
        total.tracks,
        total.bad_values,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() || total.bad_values > 0 {
        bail!("{} errors, {} bad values", total.errors.len(), total.bad_values);
    }
    Ok(())
}

/// Reads every GTA IV collision file (`.wbn`, `.wbd`) and the collision of
/// every fragment (`.wft`) and builds their meshes.
pub(crate) fn cmd_verify_gta4_collisions(game_dir: &Path, within: &str) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        files: usize,
        bounds: usize,
        triangles: usize,
        primitives: usize,
        materials: std::collections::BTreeSet<u8>,
        errors: Vec<String>,
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = Stats::default();
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                let lower = e.name.to_ascii_lowercase();
                let fragment = lower.ends_with(".wft");
                if e.resource_type().is_none() || !(lower.ends_with(".wbn") || lower.ends_with(".wbd") || fragment) {
                    return;
                }
                let parsed = a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| {
                    use rage_formats::wbn;
                    Ok(if fragment {
                        // The collision a fragment keeps for itself, if it has one.
                        match rage_formats::wdr::fragment_bound(&r) {
                            Some(at) => vec![wbn::parse_bound(&r, at)?.1],
                            None => Vec::new(),
                        }
                    } else if lower.ends_with(".wbn") {
                        vec![wbn::parse_wbn(&r)?.1]
                    } else {
                        wbn::parse_wbd(&r)?.into_iter().map(|(_, _, st)| st).collect()
                    })
                });
                if !matches!(&parsed, Ok(list) if list.is_empty()) {
                    s.files += 1;
                }
                match parsed {
                    Ok(list) => {
                        for st in list {
                            s.bounds += st.bounds;
                            s.triangles += st.triangles;
                            s.primitives += st.primitives;
                            s.materials.extend(st.materials);
                        }
                    }
                    Err(err) => s.errors.push(format!("{p}: {err:#}")),
                }
            });
            s
        })
        .collect();
    let mut total = Stats { errors: open_errors, ..Default::default() };
    for s in all {
        total.files += s.files;
        total.bounds += s.bounds;
        total.triangles += s.triangles;
        total.primitives += s.primitives;
        total.materials.extend(s.materials);
        total.errors.extend(s.errors);
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} collision files, {} bounds, {} triangles, {} spheres/capsules, {} material types, {} errors ({:.1}s)",
        total.files,
        total.bounds,
        total.triangles,
        total.primitives,
        total.materials.len(),
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}

/// Parses every GTA IV model (`.wdr`, `.wdd`, `.wft`).
pub(crate) fn cmd_verify_gta4_models(game_dir: &Path, within: &str) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        files: usize,
        drawables: usize,
        meshes: usize,
        triangles: usize,
        textured: usize,
        shaders: usize,
        errors: Vec<String>,
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = Stats::default();
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                let lower = e.name.to_ascii_lowercase();
                let kind = ["wdr", "wdd", "wft"].into_iter().find(|x| lower.ends_with(&format!(".{x}")));
                let (Some(kind), Some(_)) = (kind, e.resource_type()) else { return };
                s.files += 1;
                let parsed = a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| {
                    use rage_formats::wdr;
                    Ok(match kind {
                        "wdr" => vec![wdr::parse_wdr(&r)?],
                        "wdd" => wdr::parse_wdd(&r)?.into_iter().map(|(_, d)| d).collect(),
                        _ => {
                            let f = wdr::parse_wft(&r)?;
                            std::iter::once(f.drawable).chain(f.children.into_iter().map(|c| c.drawable)).collect()
                        }
                    })
                });
                match parsed {
                    Ok(ds) => {
                        for d in ds {
                            s.drawables += 1;
                            s.meshes += d.meshes.len();
                            s.triangles += d.triangle_count();
                            s.shaders += d.shaders.len();
                            s.textured += d.shaders.iter().filter(|sh| sh.base_texture().is_some()).count();
                        }
                    }
                    Err(err) => s.errors.push(format!("{p}: {err:#}")),
                }
            });
            s
        })
        .collect();
    let mut total = Stats { errors: open_errors, ..Default::default() };
    for s in all {
        total.files += s.files;
        total.drawables += s.drawables;
        total.meshes += s.meshes;
        total.triangles += s.triangles;
        total.shaders += s.shaders;
        total.textured += s.textured;
        total.errors.extend(s.errors);
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} model files, {} drawables, {} meshes, {} triangles, {}/{} shaders with a colour texture, {} errors ({:.1}s)",
        total.files,
        total.drawables,
        total.meshes,
        total.triangles,
        total.textured,
        total.shaders,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}

/// Reads every GTA IV text table (`.gxt`: to text and back, identical), every
/// placement file (`.wpl`, on disk and in archives) and every script header (`.sco`).
pub(crate) fn cmd_verify_gta4_text(game_dir: &Path, within: &str) -> Result<()> {
    let started = Instant::now();
    let dir = match vfs::resolve(game_dir, within, None)? {
        Node::Dir(d) => d,
        _ => bail!("{within} is not a folder"),
    };
    let mut disk = Vec::new();
    collect_archives(&dir, &["gxt", "wpl"], &mut disk);
    let mut errors = Vec::new();
    let (mut tables, mut strings, mut placements, mut records, mut scripts, mut lossy) = (0, 0, 0, 0, 0, 0);
    for p in &disk {
        let rel = p.strip_prefix(game_dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let data = fs::read(p)?;
        if rel.to_ascii_lowercase().ends_with(".gxt") {
            let check = rage_formats::gxt::parse(&data).map_err(anyhow::Error::from).and_then(|t| {
                let text = rage_formats::gxt::to_text(&t, |_| None);
                let back = rage_formats::gxt::parse(&rage_formats::gxt::write(&rage_formats::gxt::from_text(&text)?))?;
                anyhow::ensure!(back == t, "text form does not convert back identically");
                Ok(t)
            });
            match check {
                Ok(t) => {
                    tables += t.len();
                    strings += t.iter().map(|t| t.entries.len()).sum::<usize>();
                }
                Err(e) => errors.push(format!("{rel}: {e:#}")),
            }
        } else {
            placements += 1;
            match openrage_app::gta4::wpl_round_trip(Some(game_dir), &data) {
                Ok((n, l)) => {
                    records += n;
                    lossy += l;
                }
                Err(e) => errors.push(format!("{rel}: {e:#}")),
            }
        }
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    errors.extend(open_errors);
    for (prefix, a, root) in &starts {
        vfs::walk_gta4(a, *root, prefix, &mut |id, p| {
            let lower = a.entry(id).name.to_ascii_lowercase();
            if lower.ends_with(".wpl") {
                placements += 1;
                match a
                    .read(id)
                    .map_err(anyhow::Error::from)
                    .and_then(|d| openrage_app::gta4::wpl_round_trip(Some(game_dir), &d))
                {
                    Ok((n, l)) => {
                        records += n;
                        lossy += l;
                    }
                    Err(e) => errors.push(format!("{p}: {e:#}")),
                }
            } else if lower.ends_with(".sco") {
                scripts += 1;
                if a.read(id).ok().and_then(|d| rage_formats::sco::header(&d)).is_none() {
                    errors.push(format!("{p}: not a script"));
                }
            }
        });
    }
    for e in errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "placement files: text converts back byte for byte; {lossy} records keep leftover bytes only through the original"
    );
    outln!(
        "{tables} text tables ({strings} strings), {placements} placement files ({records} records), {scripts} scripts, {} errors ({:.1}s)",
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

/// Rebuilds the table of every GTA IV package (`.rpf`) in memory and compares
/// it with the stored one: the editor writes tables exactly like the game's.
pub(crate) fn cmd_verify_gta4_tables(game_dir: &Path, within: &str) -> Result<()> {
    let root = game_dir.join(within.trim_start_matches('/'));
    let mut found = Vec::new();
    if root.is_file() {
        found.push(root);
    } else {
        collect_archives(&root, &["rpf"], &mut found);
    }
    let key = rage_rpf::gta4::default_key();
    let started = Instant::now();
    let results: Vec<(String, Result<bool, String>)> = found
        .par_iter()
        .filter(|p| rage_rpf::Gta4Archive::file_is_gta4(p))
        .map(|p| {
            let rel = p.strip_prefix(game_dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
            let r = rage_rpf::gta4_rpf_editor::rebuild_toc(p, key.as_deref())
                .map(|(stored, rebuilt)| stored == rebuilt)
                .map_err(|e| e.to_string());
            (rel, r)
        })
        .collect();
    let mut failed = 0;
    for (rel, r) in &results {
        match r {
            Ok(true) => {}
            Ok(false) => {
                failed += 1;
                outln!("FAIL {rel}: rebuilt table differs");
            }
            Err(e) => {
                failed += 1;
                outln!("FAIL {rel}: {e}");
            }
        }
    }
    outln!(
        "{} packages, {} tables rebuilt byte for byte, {failed} errors ({:.1}s)",
        results.len(),
        results.len() - failed,
        started.elapsed().as_secs_f32()
    );
    if failed > 0 {
        bail!("{failed} errors");
    }
    Ok(())
}

/// Parses every GTA IV texture dictionary (`.wtd`) and decodes every texture.
pub(crate) fn cmd_verify_gta4_textures(game_dir: &Path, within: &str, rebuild: bool) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        dicts: usize,
        textures: usize,
        decoded: usize,
        rebuilt: usize,
        models: usize,
        identical: usize,
        formats: std::collections::BTreeMap<String, usize>,
        errors: Vec<String>,
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = Stats::default();
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                let lower = e.name.to_ascii_lowercase();
                // Texture dictionaries, and models (.wdr) with textures of their own.
                let model = lower.ends_with(".wdr");
                if e.resource_type().is_none() || !(lower.ends_with(".wtd") || model) {
                    return;
                }
                let read = a.read(id).map_err(anyhow::Error::from).and_then(|raw| {
                    let header = rage_resource::rsc5_header(&raw).context("no RSC5 header")?;
                    let res = rage_resource::Resource::from_rsc5(&raw)?;
                    let root = match model {
                        true => {
                            rage_formats::wdr::embedded_textures(&res, rage_resource::Resource::ROOT).map(|e| e.root)
                        }
                        false => Some(rage_resource::Resource::ROOT),
                    };
                    let dict = root.map(|r| rage_formats::ytd::parse_gta4_at(&res, r)).transpose()?;
                    Ok((header, res, root.zip(dict)))
                });
                let ((kind, flags), res, (root, dict)) = match read {
                    Ok((h, r, Some(d))) => (h, r, d),
                    Ok((_, _, None)) => return,
                    Err(err) => return s.errors.push(format!("{p}: {err:#}")),
                };
                s.dicts += 1;
                s.models += model as usize;
                s.errors.extend(dict.errors.iter().map(|err| format!("{p}: {err}")));
                if rebuild && dict.errors.is_empty() {
                    // The writer's rebuild must read back to the same textures.
                    let mut same_memory = false;
                    let check = if model {
                        rage_formats::wtd_write::rebuild_embedded(&res, kind, flags, root, &dict)
                            .and_then(|file| rage_resource::Resource::from_rsc5(&file))
                            .and_then(|r| {
                                // Nothing changed: the model must come out byte for byte.
                                same_memory = r.system == res.system && r.graphics == res.graphics;
                                rage_formats::ytd::parse_gta4_at(&r, root)
                            })
                    } else {
                        rage_formats::wtd_write::rebuild(&res, kind, flags, &dict)
                            .and_then(|file| rage_resource::Resource::from_rsc5(&file))
                            .and_then(|r| rage_formats::ytd::parse_gta4(&r))
                    };
                    s.identical += same_memory as usize;
                    let key = |d: &rage_formats::ytd::TextureDictionary| {
                        let mut v: Vec<_> = d
                            .textures
                            .iter()
                            .map(|t| (t.name.clone(), t.width, t.height, t.levels, t.format, t.data.clone()))
                            .collect();
                        v.sort_by(|x, y| x.0.cmp(&y.0));
                        v
                    };
                    match check {
                        Ok(back) if back.errors.is_empty() && key(&back) == key(&dict) => s.rebuilt += 1,
                        Ok(_) => s.errors.push(format!("{p}: rebuilt dictionary differs")),
                        Err(e) => s.errors.push(format!("{p}: rebuild failed: {e}")),
                    }
                }
                for t in &dict.textures {
                    s.textures += 1;
                    *s.formats.entry(t.format.name()).or_default() += 1;
                    match t.to_rgba() {
                        Some(_) => s.decoded += 1,
                        None => s.errors.push(format!("{p}/{}: cannot decode {}", t.name, t.format.name())),
                    }
                }
            });
            s
        })
        .collect();
    let mut total = Stats { errors: open_errors, ..Default::default() };
    for s in all {
        total.dicts += s.dicts;
        total.textures += s.textures;
        total.decoded += s.decoded;
        total.rebuilt += s.rebuilt;
        total.models += s.models;
        total.identical += s.identical;
        total.errors.extend(s.errors);
        for (k, v) in s.formats {
            *total.formats.entry(k).or_default() += v;
        }
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    let formats: Vec<String> = total.formats.iter().map(|(f, n)| format!("{f} {n}")).collect();
    outln!("formats: {}", formats.join(", "));
    if rebuild {
        outln!("{} dictionaries rebuilt and read back identically", total.rebuilt);
        outln!(
            "{} of {} models with textures rebuilt byte for byte (unchanged textures keep their place)",
            total.identical,
            total.models
        );
    }
    outln!(
        "{} texture dictionaries ({} in models), {} textures, {} decoded, {} errors ({:.1}s)",
        total.dicts,
        total.models,
        total.textures,
        total.decoded,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}
