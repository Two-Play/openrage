//! Reading and converting single files: models, maps, animations, audio, metadata, scripts, text.

use crate::*;

/// What `openrage model` writes besides the listing.
pub(crate) struct ModelOutput<'a> {
    pub(crate) export: Option<&'a Path>,
    pub(crate) render: Option<&'a Path>,
    pub(crate) yaw: f32,
    pub(crate) anim: Option<&'a str>,
}

pub(crate) fn cmd_model(
    game_dir: &Path,
    path: &str,
    outfit: Option<usize>,
    output: ModelOutput,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let ModelOutput { export, render, yaw, anim } = output;
    let model = match outfit {
        Some(n) => {
            let name = path.rsplit('/').next().unwrap_or(path);
            let stem = name.split('.').next().unwrap_or(name);
            openrage_app::models::load_rdr2_ped(game_dir, stem, n)?
        }
        None => openrage_app::models::load(game_dir, keys.clone(), path)?,
    };
    if let Some(p) = &model.ped {
        outln!("{}: outfit {} of {}", p.name, p.outfit, p.outfits);
    }
    if let Some(s) = &model.skeleton {
        outln!("skeleton: {} bones", s.bones.len());
    }
    for (name, d) in &model.drawables {
        outln!(
            "{name}: {} meshes, {} vertices, {} triangles, {} shaders",
            d.meshes.len(),
            d.vertex_count(),
            d.triangle_count(),
            d.shaders.len()
        );
        for (i, s) in d.shaders.iter().enumerate() {
            let base = s.base_texture().unwrap_or("-");
            let found = if base == "-" || model.texture(d, base).is_some() { "" } else { " (not found)" };
            outln!("  shader {i:>2} {:08x}  base {base}{found}  [{}]", s.name_hash, s.textures.join(", "));
        }
    }
    for (name, dict) in &model.external {
        outln!("textures from {name}: {}", dict.textures.len());
    }
    let shown = model.parts.iter().filter(|p| p.visible).count();
    outln!("{} parts ({shown} shown):", model.parts.len());
    for p in &model.parts {
        let [x, y, z] = [p.transform[12], p.transform[13], p.transform[14]];
        let mark = if p.visible { "*" } else { " " };
        outln!(
            "  {mark} {:<20} {:<12} at ({x:.2}, {y:.2}, {z:.2})  {}",
            p.name,
            p.group,
            model.drawables[p.drawable].0
        );
    }
    outln!("{} materials:", model.materials.len());
    for m in &model.materials {
        let kind = if m.paint {
            "paint"
        } else if m.textured {
            "textured"
        } else {
            "plain"
        };
        outln!("  {:<24} {kind}", m.label);
    }
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("model");
    if let Some(out) = export {
        let glb =
            openrage_app::models::to_glb(&model, stem, &model.default_look()).context("model has nothing to export")?;
        fs::write(out, &glb).with_context(|| format!("writing {}", out.display()))?;
        outln!("Wrote {} ({})", out.display(), human(glb.len() as u64));
    }
    if let Some(out) = render {
        use openrage_app::render;
        let materials = render::Materials::new(&model);
        let view = render::View { yaw: yaw.to_radians(), ..Default::default() };
        let started = Instant::now();
        let look = model.default_look();
        let skin = match anim {
            Some(spec) => {
                let (ycd, rest) = spec.rsplit_once(':').context("--anim wants <ycd>:<clip>[@seconds]")?;
                let (clip, secs) = match rest.split_once('@') {
                    Some((c, t)) => (c, t.parse::<f32>().context("bad time")?),
                    None => (rest, 0.0),
                };
                // A cutscene (.cut) plays its sections joined into one clip per actor.
                let dict = openrage_app::anim::load_clips(game_dir, keys, ycd)?;
                let c = dict.clips.iter().find(|c| c.name.eq_ignore_ascii_case(clip)).context("no such clip")?;
                let (_, part, at) = c.part_at(secs).context("clip without animation")?;
                let a = &dict.animations[part.animation];
                let skel = model.skeleton.as_ref().context("the model has no skeleton")?;
                let binding = openrage_app::anim::Binding::new(skel, a).with_expressions(skel, a, &model.expressions);
                outln!(
                    "animating {} of {} bones ({:.1} s clip)",
                    binding.animated_bones(),
                    skel.bones.len(),
                    c.duration()
                );
                Some(binding.skin(skel, a, a.frame_at(at)))
            }
            None => None,
        };
        let (lo, hi) = render::bounds(&model, &look.parts).context("nothing to render")?;
        let camera = render::orbit_camera(lo, hi, view);
        let rgba = render::render_camera(&model, &look, &materials, &camera, skin.as_deref(), 960, 600);
        let ms = started.elapsed().as_millis();
        fs::write(out, rage_formats::png(960, 600, &rgba)?)?;
        outln!("Rendered {} in {ms} ms", out.display());
    }
    Ok(())
}

pub(crate) fn cmd_map(
    game_dir: &Path,
    path: Option<&str>,
    render: Option<&Path>,
    yaw: f32,
    export: Option<&Path>,
    check: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use openrage_app::world;
    let started = Instant::now();
    let w = world::load(game_dir, keys.clone())?;
    let files = world::files(game_dir)?;
    if check {
        return check_maps(game_dir, &w, &files, keys, started);
    }
    let path = path.context("which map file? (or --check for all)")?;
    let scene = world::load_map(&w, &files, game_dir, keys, path, &|done, total| {
        if done % 25 == 0 || done == total {
            eprint!("\rloading models: {done}/{total}");
        }
    })?;
    eprintln!();
    let placed: usize = scene.models.iter().map(|(_, _, t)| t.len()).sum();
    outln!(
        "{}: {} objects, {} models loaded ({placed} placed), {} archetypes without a model ({:.1}s)",
        path,
        scene.objects,
        scene.models.len(),
        scene.missing.len(),
        started.elapsed().as_secs_f32()
    );
    for (name, n) in scene.missing.iter().take(10) {
        outln!("  no model: {n:>3} × {name}");
    }
    let looks: Vec<openrage_app::models::Look> = scene.models.iter().map(|(_, m, _)| m.default_look()).collect();
    let items: Vec<(&openrage_app::models::LoadedModel, &openrage_app::models::Look, &[rage_formats::math::Mat4])> =
        scene.models.iter().zip(&looks).map(|((_, m, t), l)| (m, l, t.as_slice())).collect();
    let (lo, hi) = world::scene_bounds(&items).context("nothing to show")?;
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
    outln!(
        "extent {:.0} × {:.0} × {:.0} m around ({:.0}, {:.0}, {:.0})",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        center[0],
        center[1],
        center[2]
    );
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("map");
    if let Some(out) = export {
        let glb = openrage_app::models::scene_to_glb(stem, &items, center).context("nothing to export")?;
        fs::write(out, &glb)?;
        outln!("Wrote {} ({})", out.display(), human(glb.len() as u64));
    }
    if let Some(out) = render {
        use openrage_app::render;
        let materials: Vec<render::Materials> =
            scene.models.iter().map(|(_, m, _)| render::Materials::new(m)).collect();
        let mut soft = Vec::new();
        for (((_, m, ts), l), mat) in scene.models.iter().zip(&looks).zip(&materials) {
            for t in ts {
                soft.push(render::SceneItem { model: m, look: l, materials: mat, transform: *t, skin: None });
            }
        }
        let camera = render::orbit_camera(lo, hi, render::View { yaw: yaw.to_radians(), ..Default::default() });
        let rgba = render::render_scene(&soft, &camera, 1280, 800);
        fs::write(out, rage_formats::png(1280, 800, &rgba)?)?;
        outln!("Rendered {}", out.display());
    }
    Ok(())
}

/// A GTA IV placement file (`.wpl`) as a scene: summary, glTF export, picture.
/// With `check`, every placement file of the game: how many objects have a model.
pub(crate) fn cmd_map_gta4(
    game_dir: &Path,
    path: Option<&str>,
    render: Option<&Path>,
    yaw: f32,
    export: Option<&Path>,
    check: bool,
) -> Result<()> {
    use openrage_app::gta4;
    let started = Instant::now();
    if check {
        return check_maps_gta4(game_dir, started);
    }
    let path = path.context("which placement file (.wpl)? (or --check for all)")?;
    let scene = gta4::load_map(game_dir, path, &|done, total| {
        if done % 25 == 0 || done == total {
            eprint!("\rloading models: {done}/{total}");
        }
    })?;
    eprintln!();
    let placed: usize = scene.models.iter().map(|(_, _, t)| t.len()).sum();
    outln!(
        "{path}: {} objects, {} models loaded ({placed} placed), {} models not found ({:.1}s)",
        scene.objects,
        scene.models.len(),
        scene.missing.len(),
        started.elapsed().as_secs_f32()
    );
    for (name, n) in scene.missing.iter().take(10) {
        outln!("  no model: {n:>3} × {name}");
    }
    let looks: Vec<openrage_app::models::Look> = scene.models.iter().map(|(_, m, _)| m.default_look()).collect();
    let items: Vec<(&openrage_app::models::LoadedModel, &openrage_app::models::Look, &[rage_formats::math::Mat4])> =
        scene.models.iter().zip(&looks).map(|((_, m, t), l)| (m, l, t.as_slice())).collect();
    let (lo, hi) = openrage_app::world::scene_bounds(&items).context("nothing to show")?;
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
    outln!(
        "extent {:.0} × {:.0} × {:.0} m around ({:.0}, {:.0}, {:.0})",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        center[0],
        center[1],
        center[2]
    );
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("map");
    if let Some(out) = export {
        let glb = openrage_app::models::scene_to_glb(stem, &items, center).context("nothing to export")?;
        fs::write(out, &glb)?;
        outln!("Wrote {} ({})", out.display(), human(glb.len() as u64));
    }
    if let Some(out) = render {
        use openrage_app::render;
        let materials: Vec<render::Materials> =
            scene.models.iter().map(|(_, m, _)| render::Materials::new(m)).collect();
        let mut soft = Vec::new();
        for (((_, m, ts), l), mat) in scene.models.iter().zip(&looks).zip(&materials) {
            for t in ts {
                soft.push(render::SceneItem { model: m, look: l, materials: mat, transform: *t, skin: None });
            }
        }
        let camera = render::orbit_camera(lo, hi, render::View { yaw: yaw.to_radians(), ..Default::default() });
        let rgba = render::render_scene(&soft, &camera, 1280, 800);
        fs::write(out, rage_formats::png(1280, 800, &rgba)?)?;
        outln!("Rendered {}", out.display());
    }
    Ok(())
}

/// Every GTA IV placement file: how many of its objects name a model the
/// game's `.ide` files define and an archive holds.
pub(crate) fn check_maps_gta4(game_dir: &Path, started: Instant) -> Result<()> {
    use openrage_app::gta4;
    let index = gta4::texture_index(game_dir);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut disk = Vec::new();
    collect_archives(game_dir, &["wpl"], &mut disk);
    for p in disk {
        let rel = p.strip_prefix(game_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        files.push((rel, fs::read(&p)?));
    }
    let (starts, _) = gta4_starts(game_dir, "")?;
    for (prefix, a, root) in &starts {
        vfs::walk_gta4(a, *root, prefix, &mut |id, p| {
            if a.entry(id).name.to_ascii_lowercase().ends_with(".wpl")
                && let Ok(d) = a.read(id)
            {
                files.push((p.to_owned(), d));
            }
        });
    }
    let (mut objects, mut unnamed, mut no_file, mut interiors) = (0usize, 0usize, 0usize, 0usize);
    let mut missing: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut check = |name: &str| {
        objects += 1;
        // Without a name (counted by the caller) or without a file.
        if !name.is_empty() && index.model_source(name, game_dir, game_dir).is_none() {
            no_file += 1;
            *missing.entry(name.to_owned()).or_default() += 1;
        }
    };
    for (_, data) in &files {
        let Ok(wpl) = rage_formats::wpl::parse(data) else { continue };
        for i in &wpl.instances {
            // An interior: the objects inside it.
            if let Some(inside) = index.mlo_entities(i.model_hash) {
                interiors += 1;
                inside.iter().for_each(|e| check(&e.model));
                continue;
            }
            match index.model_name(i.model_hash) {
                None => {
                    check("");
                    unnamed += 1;
                }
                Some(n) => check(&n),
            }
        }
    }
    let mut missing: Vec<(String, usize)> = missing.into_iter().collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (name, n) in missing.iter().take(15) {
        outln!("  no model file: {n:>4} × {name}");
    }
    outln!(
        "{} placement files, {objects} objects ({interiors} interiors expanded): {} with a model, {unnamed} with an unknown model name, {no_file} whose model was not found ({:.1}s)",
        files.len(),
        objects - unnamed - no_file,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Every map file: how many of its objects have a model.
pub(crate) fn check_maps(
    game_dir: &Path,
    w: &openrage_app::world::World,
    files: &openrage_app::world::Files,
    keys: Option<Arc<GtaVKeys>>,
    started: Instant,
) -> Result<()> {
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let done = AtomicUsize::new(0);
    let total = w.maps.len();
    let results: Vec<Option<(usize, Vec<String>)>> = w
        .maps
        .par_iter()
        .map(|(vpath, _, _)| {
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(500) || n == total {
                eprint!("\r{n}/{total} map files");
            }
            let xml = openrage_app::world::read_map(game_dir, keys.clone(), vpath)?;
            let placed = openrage_app::world::scene_entities(w, &xml);
            let missing = placed
                .iter()
                .filter(|p| !files.models.contains_key(&p.archetype))
                .map(|p| p.archetype.clone())
                .collect();
            Some((placed.len(), missing))
        })
        .collect();
    eprintln!();
    let unreadable = results.iter().filter(|r| r.is_none()).count();
    let (mut objects, mut missing_n) = (0usize, 0usize);
    let mut missing: HashMap<String, usize> = HashMap::new();
    let mut empty = 0;
    for (n, m) in results.into_iter().flatten() {
        objects += n;
        if n == 0 {
            empty += 1;
        }
        missing_n += m.len();
        for a in m {
            *missing.entry(a).or_default() += 1;
        }
    }
    outln!(
        "{total} map files ({unreadable} unreadable, {empty} without objects to show), {objects} objects; \
         {missing_n} objects ({} archetypes) without a model",
        missing.len()
    );
    let mut top: Vec<(String, usize)> = missing.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    for (name, n) in top.iter().take(15) {
        outln!("  {n:>5} × {name}");
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f32());
    Ok(())
}

pub(crate) fn cmd_clips(game_dir: &Path, path: &str, clip: Option<&str>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let dict = openrage_app::anim::load_clips(game_dir, keys, path)?;
    outln!("{} clips, {} animations", dict.clips.len(), dict.animations.len());
    for c in &dict.clips {
        let parts: Vec<String> = c
            .parts
            .iter()
            .map(|p| {
                let a = &dict.animations[p.animation];
                format!("anim {} ({} frames, {} tracks)", p.animation, a.frames, a.tracks.len())
            })
            .collect();
        outln!("  {:<40} {:>6.2}s  {}", c.name, c.duration(), parts.join(", "));
    }
    if let Some(name) = clip {
        let c = dict.clips.iter().find(|c| c.name.eq_ignore_ascii_case(name)).context("no such clip")?;
        let a = &dict.animations[c.parts.first().context("clip without animation")?.animation];
        let mut worst_norm = 0f32;
        let mut worst_jump = 0f32;
        for (t, tr) in a.tracks.iter().enumerate() {
            let vals: Vec<[f32; 4]> = (0..a.frames).filter_map(|f| a.value(t, f as f32)).collect();
            if tr.format == 1 {
                for v in &vals {
                    let n = (v.iter().map(|x| x * x).sum::<f32>()).sqrt();
                    worst_norm = worst_norm.max((n - 1.0).abs());
                }
            }
            for w in vals.windows(2) {
                let d = (0..3).map(|i| (w[1][i] - w[0][i]).abs()).fold(0.0, f32::max);
                worst_jump = worst_jump.max(d);
            }
            if t < 12 {
                let first = vals.first().copied().unwrap_or_default();
                let mid = vals.get(vals.len() / 2).copied().unwrap_or_default();
                outln!(
                    "  bone {:04x} track {} fmt {}: f0 [{:.3} {:.3} {:.3} {:.3}]  mid [{:.3} {:.3} {:.3} {:.3}]",
                    tr.bone,
                    tr.track,
                    tr.format,
                    first[0],
                    first[1],
                    first[2],
                    first[3],
                    mid[0],
                    mid[1],
                    mid[2],
                    mid[3]
                );
            }
        }
        outln!("worst |q|-1: {worst_norm:.4}, largest frame-to-frame change: {worst_jump:.4}");
    }
    Ok(())
}

pub(crate) fn cmd_audio(game_dir: &Path, path: &str, export: Option<&Path>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let awc = openrage_app::audio::load(game_dir, keys, path)?;
    outln!("flags {:#06x}, {} streams{}", awc.flags, awc.streams.len(), if awc.encrypted { ", encrypted" } else { "" });
    if let Some(f) = &awc.stream_format {
        outln!(
            "multi-channel stream: {} channels, {} blocks of {}",
            f.channels,
            f.block_count,
            human(f.block_size as u64)
        );
    }
    for s in &awc.sounds {
        outln!(
            "  {:08x}  {:>8.2}s  {:>6} Hz  {}{}",
            s.id,
            s.duration_secs(),
            s.sample_rate,
            s.codec.name(),
            s.channel.map(|c| format!("  channel {c}")).unwrap_or_default()
        );
    }
    for (i, (id, chunks)) in awc.streams.iter().enumerate().take(4) {
        for c in chunks {
            let head = &awc.data[c.offset..(c.offset + 32).min(c.offset + c.size)];
            outln!("  stream {i} {id:08x} chunk {:02x} {:>8} @ {:>8}: {}", c.kind, c.size, c.offset, hex(head));
        }
    }
    if let Some(dir) = export {
        fs::create_dir_all(dir)?;
        let stem = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".awc");
        let started = Instant::now();
        if awc.stream_format.is_some() {
            let pcm = awc.decode_stream()?;
            let out = dir.join(format!("{stem}.wav"));
            fs::write(&out, pcm.to_wav())?;
            outln!("Wrote {} ({:.1}s, {} channels)", out.display(), pcm.duration_secs(), pcm.channels);
        } else {
            for (i, s) in awc.sounds.iter().enumerate() {
                let pcm = awc.decode_sound(i)?;
                let out = dir.join(format!("{stem}_{:08x}.wav", s.id));
                fs::write(&out, pcm.to_wav())?;
                outln!("Wrote {} ({:.2}s)", out.display(), pcm.duration_secs());
            }
        }
        outln!("in {} ms", started.elapsed().as_millis());
    }
    Ok(())
}

pub(crate) fn cmd_xml(
    game_dir: &Path,
    path: &str,
    out: Option<&Path>,
    tree: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    // Names of models, textures, … from the search index, so hashes read as names.
    if let Ok(names) = open_index(game_dir).and_then(|i| Ok(i.known_names()?)) {
        rage_formats::names::register(names);
    }
    let xml = openrage_app::metadata::load(game_dir, keys, path)?;
    for e in xml.errors.iter().take(20) {
        eprintln!("warning: {e}");
    }
    if tree {
        fn print(n: &openrage_app::xmltree::Node, depth: usize) {
            outln!("{}{}", "  ".repeat(depth), n.label);
            for (k, v) in &n.fields {
                outln!("{}  · {k}: {v}", "  ".repeat(depth));
            }
            for c in &n.children {
                print(c, depth + 1);
            }
        }
        let started = Instant::now();
        let root = openrage_app::xmltree::parse(&xml.text)?;
        print(&root, 0);
        eprintln!(
            "{} structures, {} of XML, tree built in {} ms",
            root.count(),
            human(xml.text.len() as u64),
            started.elapsed().as_millis()
        );
        return Ok(());
    }
    match out {
        Some(file) => {
            fs::write(file, &xml.text)?;
            outln!(
                "Wrote {} ({}, {}{})",
                file.display(),
                xml.encoding.name(),
                xml.root.as_deref().unwrap_or("?"),
                if xml.errors.is_empty() { String::new() } else { format!(", {} problems", xml.errors.len()) }
            );
        }
        None => outln!("{}", xml.text.trim_end()),
    }
    Ok(())
}

pub(crate) fn cmd_media(game_dir: &Path, path: &str, swf: Option<&Path>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    let data = match node {
        Node::ArchiveFile(rpf, id) => rpf.read(id)?,
        Node::PlainFile(p) => fs::read(p)?,
        _ => bail!("{path} is not a file"),
    };
    for (k, v) in openrage_app::media::rows(path, &data)? {
        outln!("{k:<14}{v}");
    }
    if let Some(out) = swf {
        fs::write(out, rage_formats::media::gfx_to_swf(&data)?)?;
        outln!("Wrote {}", out.display());
    }
    Ok(())
}

pub(crate) fn cmd_script(
    game_dir: &Path,
    path: &str,
    out: Option<&Path>,
    strings: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    if let Node::Gta4File(a, id) = &node {
        let data = a.read(*id)?;
        let text = if strings {
            openrage_app::gta4::read_sco(&data)?.strings().join("\n") + "\n"
        } else {
            openrage_app::gta4::sco_listing(&a.entry(*id).name, &data)?
        };
        match out {
            Some(file) => {
                fs::write(file, &text)?;
                outln!("Wrote {}", file.display());
            }
            None => outln!("{}", text.trim_end()),
        }
        return Ok(());
    }
    let Node::ArchiveFile(rpf, id) = node else { bail!("{path} is not a file inside an archive") };
    let s = rage_formats::ysc::parse(&rpf.read_resource(id)?)?;
    let text = if strings {
        s.all_strings().join("\n") + "\n"
    } else {
        let header = format!(
            "// {}: {} of code, {} natives, {} statics, {} strings, opcode table {}\n",
            s.name,
            human(s.code.len() as u64),
            s.natives.len(),
            s.statics.len(),
            s.all_strings().len(),
            s.table.map_or("unknown (jumps do not line up)".to_owned(), |t| format!("{t:?}"))
        );
        header + &s.listing(openrage_app::natives::name).map_err(|e| anyhow::anyhow!(e))?
    };
    match out {
        Some(file) => {
            fs::write(file, &text)?;
            outln!("Wrote {}", file.display());
        }
        None => outln!("{}", text.trim_end()),
    }
    Ok(())
}

pub(crate) fn cmd_particles(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    let Node::ArchiveFile(rpf, id) = node else { bail!("{path} is not a file inside an archive") };
    let p = rage_formats::ypt::parse(&rpf.read_resource(id)?)?;
    outln!(
        "{}: {} effects, {} emitters, {} particle rules",
        p.name,
        p.effects.len(),
        p.emitters.len(),
        p.particles.len()
    );
    match &p.textures {
        Some(t) => outln!("{} textures", t.textures.len()),
        None => outln!("no textures"),
    }
    let tris: usize = p.drawables.iter().map(|(_, d)| d.triangle_count()).sum();
    outln!("{} models ({tris} triangles)", p.drawables.len());
    for e in &p.errors {
        outln!("problem: {e}");
    }
    for e in &p.effects {
        outln!("  effect  {e}");
    }
    Ok(())
}

pub(crate) fn cmd_text(game_dir: &Path, path: &str, out: Option<&Path>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::texttable::{self, Kind};
    if path.to_ascii_lowercase().ends_with(".wpl") {
        let data = match rage_mods::resolve(game_dir, path, keys)?.0 {
            Node::Gta4File(a, id) => a.read(id)?,
            Node::PlainFile(p) => fs::read(p)?,
            _ => bail!("{path} is not a file"),
        };
        let (text, count) = openrage_app::gta4::wpl_to_text(Some(game_dir), &data)?;
        return match out {
            Some(file) => {
                fs::write(file, &text)?;
                outln!("Wrote {} ({count} records)", file.display());
                Ok(())
            }
            None => {
                outln!("{}", text.trim_end());
                Ok(())
            }
        };
    }
    let kind = Kind::of(path).context("not a text table (.gxt2, .nametable, .gxt) or placement file (.wpl)")?;
    if let Ok(names) = open_index(game_dir).and_then(|i| Ok(i.known_names()?)) {
        rage_formats::names::register(names);
    }
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    let data = match node {
        Node::ArchiveFile(rpf, id) => rpf.read(id)?,
        Node::Gta4File(a, id) => a.read(id)?,
        Node::PlainFile(p) => fs::read(p)?,
        _ => bail!("{path} is not a file"),
    };
    let (text, count) = texttable::to_text(kind, &data)?;
    match out {
        Some(file) => {
            fs::write(file, &text)?;
            outln!("Wrote {} ({count} entries)", file.display());
        }
        None => outln!("{}", text.trim_end()),
    }
    Ok(())
}
