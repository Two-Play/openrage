//! `openrage verify`: every file of a kind in the whole GTA V game.

use crate::*;

pub(crate) fn cmd_verify_textures(
    game_dir: &Path,
    within: &str,
    rebuild: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        dicts: usize,
        textures: usize,
        decoded: usize,
        undecodable: std::collections::BTreeMap<String, usize>,
        errors: Vec<String>,
        models: usize,
        grown: usize,
        unreadable: Vec<String>,
    }
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut s = Stats::default();
            let mut nested_errors = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    // Models with textures of their own: check the writer in memory.
                    if rebuild
                        && openrage_app::textures::gta5_model_ext(p).is_some()
                        && let rage_rpf::EntryKind::Resource { system_flags, graphics_flags, .. } = a.entry(id).kind
                    {
                        let check = a.read_resource(id).map_err(anyhow::Error::from).and_then(|res| {
                            openrage_app::textures::check_model_textures(&res, p, system_flags, graphics_flags)
                        });
                        match check {
                            Ok(openrage_app::textures::ModelCheck::Ok { grown }) => {
                                s.models += 1;
                                s.grown += grown as usize;
                            }
                            Ok(openrage_app::textures::ModelCheck::NoTextures) => {}
                            Ok(openrage_app::textures::ModelCheck::Unreadable(why)) => {
                                s.unreadable.push(format!("{p}: {why}"))
                            }
                            Err(e) => s.errors.push(format!("{p}: model textures: {e:#}")),
                        }
                        return;
                    }
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ytd")
                        || a.entry(id).resource_version().is_none()
                    {
                        return;
                    }
                    s.dicts += 1;
                    let dict = match a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|r| Ok(rage_formats::ytd::parse(&r)?))
                    {
                        Ok(d) => d,
                        Err(e) => return s.errors.push(format!("{p}: {e:#}")),
                    };
                    s.errors.extend(dict.errors.iter().map(|e| format!("{p}: {e}")));
                    if rebuild && dict.errors.is_empty() {
                        let check = rage_formats::ytd_write::rebuild(&dict)
                            .and_then(|file| rage_resource::Resource::from_rsc7(&file))
                            .and_then(|res| rage_formats::ytd::parse(&res));
                        match check {
                            Ok(back) => {
                                let mut a: Vec<_> = dict
                                    .textures
                                    .iter()
                                    .map(|t| (&t.name, t.width, t.height, t.levels, t.format, &t.data))
                                    .collect();
                                let mut b: Vec<_> = back
                                    .textures
                                    .iter()
                                    .map(|t| (&t.name, t.width, t.height, t.levels, t.format, &t.data))
                                    .collect();
                                a.sort_by(|x, y| x.0.cmp(y.0));
                                b.sort_by(|x, y| x.0.cmp(y.0));
                                if a != b || !back.errors.is_empty() {
                                    s.errors.push(format!("{p}: rebuilt dictionary differs from the original"));
                                }
                                // Structures must match byte for byte, apart from pointers.
                                let pointers: &[(usize, usize)] = match dict.layout {
                                    rage_formats::ytd::Layout::Legacy => &[(0x28, 0x30), (0x70, 0x78)],
                                    rage_formats::ytd::Layout::Enhanced | rage_formats::ytd::Layout::Rdr2 => {
                                        &[(0x28, 0x40)]
                                    }
                                    rage_formats::ytd::Layout::Gta4 => &[(0x14, 0x18), (0x48, 0x4C)],
                                };
                                let masked = |t: &[u8]| {
                                    let mut t = t.to_vec();
                                    for &(from, to) in pointers {
                                        t[from..to].fill(0);
                                    }
                                    t
                                };
                                for (t, st) in dict.textures.iter().zip(&dict.stored) {
                                    let Some(j) = back.textures.iter().position(|x| x.name == t.name) else { continue };
                                    let (x, y) = (masked(&st.template), masked(&back.stored[j].template));
                                    if x != y {
                                        let diffs: Vec<String> = (0..x.len())
                                            .filter(|&i| x[i] != y[i])
                                            .map(|i| format!("{i:#x}: {:02x}→{:02x}", x[i], y[i]))
                                            .collect();
                                        s.errors.push(format!(
                                            "{p}/{}: rebuilt texture structure differs ({}x{} {} mips {}; {})",
                                            t.name,
                                            t.width,
                                            t.height,
                                            t.format.name(),
                                            t.levels,
                                            diffs.join(", ")
                                        ));
                                        break;
                                    }
                                }
                            }
                            Err(e) => s.errors.push(format!("{p}: rebuild failed: {e}")),
                        }
                    }
                    for t in &dict.textures {
                        s.textures += 1;
                        match t.to_rgba() {
                            Some(_) => s.decoded += 1,
                            None => *s.undecodable.entry(t.format.name()).or_default() += 1,
                        }
                    }
                },
                &mut |e| nested_errors.push(e.to_string()),
            );
            s.errors.extend(nested_errors);
            s
        })
        .collect();
    let mut total = Stats::default();
    for s in all {
        total.dicts += s.dicts;
        total.textures += s.textures;
        total.decoded += s.decoded;
        total.models += s.models;
        total.grown += s.grown;
        total.unreadable.extend(s.unreadable);
        total.errors.extend(s.errors);
        for (k, v) in s.undecodable {
            *total.undecodable.entry(k).or_default() += v;
        }
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    for (format, n) in &total.undecodable {
        outln!("no preview decoder: {n} textures in {format}");
    }
    if rebuild {
        outln!(
            "{} models with textures of their own checked; a texture twice the size fitted into {} of them",
            total.models,
            total.grown
        );
        for u in &total.unreadable {
            outln!("skipped (a texture cannot be read, so it cannot be changed): {u}");
        }
    }
    outln!(
        "{} texture dictionaries, {} textures, {} decoded, {} errors ({:.1}s)",
        total.dicts,
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

pub(crate) fn cmd_verify_media(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::media;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, f32, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0.0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    if !media::is_media(&name) {
                        return;
                    }
                    let result = a.read(id).map_err(anyhow::Error::from).and_then(|d| {
                        if name.ends_with(".bik") {
                            let b = media::bink(&d)?;
                            c.1 += 1;
                            c.2 += b.duration();
                        } else {
                            let g = media::gfx(&d)?;
                            c.0 += 1;
                            c.3 += g.exports.len();
                        }
                        Ok(())
                    });
                    if let Err(e) = result {
                        errors.push(format!("{p}: {e:#}"));
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0.0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} Scaleform movies ({} exported symbols), {} Bink videos ({:.1} min); {} errors ({:.1}s)",
        t.0,
        t.3,
        t.1,
        t.2 / 60.0,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_scripts(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::ysc;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ysc") {
                        return;
                    }
                    c.0 += 1;
                    match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(ysc::parse(&r)?)) {
                        Ok(s) => match s.table {
                            Some(t) => {
                                if t == ysc::Table::Current {
                                    c.1 += 1;
                                } else {
                                    c.2 += 1;
                                }
                                c.3 += s.check().unwrap_or(0);
                                c.4 += s.code.len();
                            }
                            None => errors
                                .push(format!("{p}: no opcode table fits ({})", s.check().err().unwrap_or_default())),
                        },
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} scripts ({} current opcode table, {} classic), {} instructions, {} of code; {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        human(t.4 as u64),
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_paths(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::paths;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize, f32, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0, 0.0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    if !paths::is_path_file(&name) {
                        return;
                    }
                    c.0 += 1;
                    let ext = name.rsplit('.').next().unwrap_or_default().to_owned();
                    match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(paths::parse(&r, &ext)?)) {
                        Ok((d, s)) => {
                            c.1 += s.nodes + s.polygons;
                            c.2 += s.links;
                            c.3 += s.points;
                            c.4 += s.duration;
                            c.5 += s.skipped;
                            let bad = d
                                .meshes
                                .iter()
                                .flat_map(|m| &m.positions)
                                .any(|p| p.iter().any(|v| !v.is_finite() || v.abs() > 20000.0));
                            if bad {
                                errors.push(format!("{p}: positions out of the map"));
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0, 0.0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4, t.5 + c.5);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} path and nav mesh files, {} nodes and polygons, {} links, {} recorded points ({:.1} h of vehicle recordings, {} implausible points skipped); {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        t.4 / 3600.0,
        t.5,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_collisions(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ybn") {
                        return;
                    }
                    c.0 += 1;
                    match a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|r| Ok(rage_formats::ybn::parse(&r)?))
                    {
                        Ok((_, s)) => {
                            c.1 += s.bounds;
                            c.2 += s.triangles;
                            c.3 += s.primitives;
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} collision files, {} bounds, {} triangles, {} primitives; {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_particles(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ypt") {
                        return;
                    }
                    c.0 += 1;
                    match a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|r| Ok(rage_formats::ypt::parse(&r)?))
                    {
                        Ok(d) => {
                            c.1 += d.effects.len();
                            c.2 += d.textures.as_ref().map_or(0, |t| t.textures.len());
                            c.3 += d.drawables.len();
                            c.4 += d.drawables.iter().map(|(_, m)| m.triangle_count()).sum::<usize>();
                            let bad_tex = d.textures.as_ref().map_or(0, |t| t.errors.len());
                            if !d.errors.is_empty() || bad_tex > 0 {
                                errors.push(format!("{p}: {} {bad_tex} texture errors", d.errors.join("; ")));
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} particle dictionaries, {} effects, {} textures, {} models ({} triangles); {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        t.4,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_text(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::texttable::{self, Kind};
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<(usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut files, mut entries, mut errors) = (0, 0, Vec::new());
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let Some(kind) = Kind::of(&a.entry(id).name) else { return };
                    files += 1;
                    let result = a.read(id).map_err(anyhow::Error::from).and_then(|data| {
                        let (text, n) = texttable::to_text(kind, &data)?;
                        let back = texttable::from_text(kind, &text)?;
                        if back != data {
                            bail!("converting back changes the file ({} → {} bytes)", data.len(), back.len());
                        }
                        Ok(n)
                    });
                    match result {
                        Ok(n) => entries += n,
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (files, entries, errors)
        })
        .collect();
    let (files, entries): (usize, usize) = all.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{files} text tables, {entries} strings and names, {} errors ({:.1}s)",
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

/// Writes every Enhanced `.ydr` anew with its own meshes (through the model
/// writer) and compares what the reader gets back.
pub(crate) fn cmd_verify_model_writer(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::{drawable, ydr_write};
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let check = |res: &rage_resource::Resource| -> Result<bool> {
        if res.version != drawable::VERSION_YDR_ENHANCED {
            return Ok(false);
        }
        let before = drawable::parse_ydr(res)?;
        let usable = ydr_write::usable_shaders(res)?;
        let meshes: Vec<ydr_write::NewMesh> = before
            .meshes
            .iter()
            .filter(|m| usable.contains(&m.shader) && !m.indices.is_empty() && m.bone_indices.is_empty())
            .map(|m| ydr_write::NewMesh {
                shader: m.shader,
                positions: m.positions.clone(),
                normals: m.normals.clone(),
                uvs: m.uvs.clone(),
                colors: m.colors.clone(),
                indices: m.indices.clone(),
            })
            .collect();
        if meshes.is_empty() {
            return Ok(false);
        }
        let file = ydr_write::replace_meshes(res, &meshes)?;
        let after = drawable::parse_ydr(&rage_resource::Resource::from_rsc7(&file)?)?;
        if after.meshes.len() != meshes.len() {
            bail!("{} meshes written, {} read back", meshes.len(), after.meshes.len());
        }
        for (i, (a, m)) in after.meshes.iter().zip(&meshes).enumerate() {
            // Bit for bit: some of the game's vertices hold NaN, which never equals itself.
            let same =
                |x: &[f32], y: &[f32]| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits());
            if !same(a.positions.as_flattened(), m.positions.as_flattened())
                || a.indices != m.indices
                || a.shader != m.shader
            {
                bail!("mesh {i}: geometry differs after writing");
            }
            if !m.uvs.is_empty() && !same(a.uvs.as_flattened(), m.uvs.as_flattened()) {
                bail!("mesh {i}: texture coordinates differ after writing");
            }
            let close = |x: &[[f32; 3]], y: &[[f32; 3]]| {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (0..3).all(|k| (p[k] - q[k]).abs() < 0.01))
            };
            if !m.normals.is_empty() && !close(&a.normals, &m.normals) {
                bail!("mesh {i}: normals differ after writing");
            }
            if !m.colors.is_empty() && a.colors.first() != m.colors.first() {
                bail!("mesh {i}: colours differ after writing");
            }
        }
        // Materials: a shader copy per mesh shader, with a texture where the
        // model has an embedded dictionary to put it in.
        // (A model with an empty embedded dictionary borrows the structure from another one when
        // importing; here only models with embedded textures get one.)
        let embedded = ydr_write::can_donate_textures(res);
        let mut materials: Vec<ydr_write::NewMaterial> = Vec::new();
        let mut with_materials = meshes.clone();
        for m in &mut with_materials {
            let k = match materials.iter().position(|mat| mat.shader == m.shader) {
                Some(k) => k,
                None => {
                    let texture = embedded
                        .then(|| {
                            ydr_write::texture_from_rgba(
                                &format!("openrage_check_{}", materials.len()),
                                2,
                                2,
                                &[200; 16],
                            )
                        })
                        .transpose()?;
                    materials.push(ydr_write::NewMaterial { shader: m.shader, texture });
                    materials.len() - 1
                }
            };
            m.shader = k as u16;
        }
        let shaders_before = before.shaders.len();
        let file = match ydr_write::replace_with_materials(res, &with_materials, &materials, None) {
            Ok(f) => f,
            // Shaders without a colour slot cannot take a texture: fine, skip that part.
            Err(e) if e.to_string().contains("colour") => return Ok(true),
            Err(e) => return Err(e.into()),
        };
        let after = drawable::parse_ydr(&rage_resource::Resource::from_rsc7(&file)?)?;
        if after.shaders.len() != shaders_before + materials.len() {
            bail!("materials: {} shaders, expected {}", after.shaders.len(), shaders_before + materials.len());
        }
        for (k, mat) in materials.iter().enumerate() {
            if let Some(t) = &mat.texture {
                if !after.shaders[shaders_before + k].textures.contains(&t.name) {
                    bail!("material {k}: its shader does not use {}", t.name);
                }
                let found = after.textures.as_ref().and_then(|d| d.textures.iter().find(|x| x.name == t.name));
                if found.and_then(|x| x.to_rgba()).as_deref() != Some(&[200u8; 16][..]) {
                    bail!("material {k}: texture {} not embedded intact", t.name);
                }
            }
        }
        if after
            .meshes
            .iter()
            .map(|m| m.shader as usize)
            .ne((0..with_materials.len()).map(|i| shaders_before + with_materials[i].shader as usize))
        {
            bail!("materials: meshes do not use their shader copies");
        }
        Ok(true)
    };
    let results: Vec<(usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut seen, mut written, mut errors, mut nested) = (0, 0, Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ydr") {
                        return;
                    }
                    seen += 1;
                    match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| check(&r)) {
                        Ok(true) => written += 1,
                        Ok(false) => {}
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (seen, written, errors)
        })
        .collect();
    let (seen, written) = results.iter().fold((0, 0), |(s, w), r| (s + r.0, w + r.1));
    let errors: Vec<&String> = results.iter().flat_map(|r| &r.2).collect();
    outln!(
        "{seen} drawables, {written} written anew and read back identically, {} errors ({:.1}s)",
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    for e in errors.iter().take(30) {
        outln!("  {e}");
    }
    if errors.is_empty() { Ok(()) } else { bail!("{} drawables failed", errors.len()) }
}

pub(crate) fn cmd_verify_meta(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::metadata;
    use std::collections::BTreeMap;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = BTreeMap<String, (usize, usize)>;
    let all: Vec<(Counts, Vec<String>, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut counts, mut errors, mut problems) = (Counts::new(), Vec::new(), Vec::new());
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let name = &a.entry(id).name;
                    if !metadata::is_metadata(name) {
                        return;
                    }
                    let ext = name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
                    let result = a.read(id).map_err(anyhow::Error::from).and_then(|d| metadata::from_bytes(&d));
                    let c = counts.entry(ext).or_insert((0, 0));
                    c.0 += 1;
                    match result {
                        Ok(x) => {
                            c.1 += x.text.len();
                            if let Some(e) = x.errors.first() {
                                problems.push(format!("{p}: {} problems, first: {e}", x.errors.len()));
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (counts, errors, problems)
        })
        .collect();
    let mut counts = BTreeMap::<String, (usize, usize)>::new();
    for (c, _, _) in &all {
        for (k, v) in c {
            let e = counts.entry(k.clone()).or_default();
            e.0 += v.0;
            e.1 += v.1;
        }
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    let problems: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    for e in problems.iter().take(30) {
        outln!("PART {e}");
    }
    for (ext, (n, bytes)) in &counts {
        outln!("  .{ext:<5} {n:>6} files, {} of XML", human(*bytes as u64));
    }
    let total: usize = counts.values().map(|c| c.0).sum();
    outln!(
        "{total} metadata files, {} errors, {} with unreadable parts ({:.1}s)",
        errors.len(),
        problems.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() || !problems.is_empty() {
        bail!("{} errors, {} partial", errors.len(), problems.len());
    }
    Ok(())
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// Per archive: files, clips, animations, frame-count mismatches, bad values, errors.
pub(crate) type AnimStats = (usize, usize, usize, usize, usize, Vec<String>);

pub(crate) fn cmd_verify_animations(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<AnimStats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut files, mut clips, mut anims, mut mismatch, mut badq) = (0, 0, 0, 0, 0);
            let mut errors = Vec::new();
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ycd")
                        || a.entry(id).resource_version().is_none()
                    {
                        return;
                    }
                    files += 1;
                    let parsed = a
                        .read_resource(id)
                        .map_err(|e| e.to_string())
                        .and_then(|r| rage_formats::clip::parse_ycd(&r).map_err(|e| e.to_string()));
                    match parsed {
                        Ok(d) => {
                            clips += d.clips.len();
                            anims += d.animations.len();
                            for an in &d.animations {
                                let blocks = an.block_frames();
                                let sum: usize = blocks.iter().sum();
                                let overlapped = sum.saturating_sub(blocks.len().saturating_sub(1));
                                if sum != an.frames && overlapped != an.frames {
                                    mismatch += 1;
                                }
                                for (t, tr) in an.tracks.iter().enumerate() {
                                    for f in (0..an.frames).step_by(7) {
                                        match an.value(t, f as f32) {
                                            Some(v) if tr.format == 1 => {
                                                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                                                if !(0.99..=1.01).contains(&n) {
                                                    badq += 1;
                                                }
                                            }
                                            Some(v) if v.iter().any(|x| !x.is_finite()) => badq += 1,
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (files, clips, anims, mismatch, badq, errors)
        })
        .collect();
    let sum = |f: fn(&AnimStats) -> usize| all.iter().map(f).sum::<usize>();
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.5).collect();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} clip dictionaries, {} clips, {} animations; {} with unexpected frame counts, {} bad values; {} errors ({:.1}s)",
        sum(|x| x.0),
        sum(|x| x.1),
        sum(|x| x.2),
        sum(|x| x.3),
        sum(|x| x.4),
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_audio(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys.clone())?;
    let started = Instant::now();
    // (containers, sounds, seconds, encrypted, not audio, errors)
    let all: Vec<(usize, usize, f64, usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut files, mut sounds, mut secs, mut encrypted, mut errors) = (0, 0, 0f64, 0, Vec::new());
            let mut not_audio = 0usize;
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !openrage_app::audio::is_audio(&a.entry(id).name) {
                        return;
                    }
                    files += 1;
                    let result = a
                        .read(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|d| openrage_app::audio::parse(d, keys.as_deref()))
                        .and_then(|awc| {
                            if awc.encrypted || awc.flags & 0xA != 0 {
                                encrypted += 1;
                            }
                            let pcms = if awc.stream_format.is_some() {
                                vec![awc.decode_stream()?]
                            } else {
                                (0..awc.sounds.len()).map(|i| awc.decode_sound(i)).collect::<Result<Vec<_>, _>>()?
                            };
                            Ok(pcms)
                        });
                    match result {
                        Ok(pcms) => {
                            sounds += pcms.len();
                            secs += pcms.iter().map(|p| p.duration_secs() as f64).sum::<f64>();
                        }
                        // A few files only share the extension (e.g. shader caches).
                        Err(e)
                            if matches!(
                                e.downcast_ref::<rage_formats::awc::AwcError>(),
                                Some(rage_formats::awc::AwcError::NotAwc)
                            ) =>
                        {
                            files -= 1;
                            not_audio += 1;
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (files, sounds, secs, encrypted, not_audio, errors)
        })
        .collect();
    let files: usize = all.iter().map(|x| x.0).sum();
    let sounds: usize = all.iter().map(|x| x.1).sum();
    let secs: f64 = all.iter().map(|x| x.2).sum();
    let encrypted: usize = all.iter().map(|x| x.3).sum();
    let not_audio: usize = all.iter().map(|x| x.4).sum();
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.5).collect();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!(
        "{files} audio containers ({encrypted} encrypted, {not_audio} other .awc files skipped), {sounds} sounds, {:.1} hours of audio, {} errors ({:.1}s)",
        secs / 3600.0,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) fn cmd_verify_models(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::drawable;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<(usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut models, mut tris, mut errors) = (0usize, 0usize, Vec::new());
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let n = a.entry(id).name.to_ascii_lowercase();
                    let drawable_file = n.ends_with(".ydr") || n.ends_with(".yft") || n.ends_with(".ydd");
                    if !drawable_file || a.entry(id).resource_version().is_none() {
                        return;
                    }
                    models += 1;
                    let parsed = a.read_resource(id).map_err(|e| e.to_string()).and_then(|r| {
                        let r = if n.ends_with(".ydd") {
                            drawable::parse_ydd(&r).map(|v| v.into_iter().map(|(_, d)| d).collect::<Vec<_>>())
                        } else if n.ends_with(".yft") {
                            drawable::parse_fragment(&r).map(|f| {
                                std::iter::once(f.drawable).chain(f.children.into_iter().map(|c| c.drawable)).collect()
                            })
                        } else {
                            drawable::parse_ydr(&r).map(|d| vec![d])
                        };
                        r.map_err(|e| e.to_string())
                    });
                    match parsed {
                        Ok(ds) => tris += ds.iter().map(|d| d.triangle_count()).sum::<usize>(),
                        Err(e) => errors.push(format!("{p}: {e}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (models, tris, errors)
        })
        .collect();
    let (models, tris): (usize, usize) = all.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!("{models} models, {tris} triangles, {} errors ({:.1}s)", errors.len(), started.elapsed().as_secs_f32());
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

pub(crate) type Start = (String, Arc<Rpf7>, EntryId);

/// Resolves `within` to the archive directories to process: either one
/// directory inside an archive, or every `.rpf` below a real directory.
pub(crate) fn archive_roots(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<Vec<Start>> {
    Ok(match vfs::resolve(game_dir, within, keys.clone())? {
        Node::ArchiveDir(rpf, dir) => vec![(within.trim_end_matches('/').to_owned(), rpf, dir)],
        Node::ArchiveFile(..) | Node::PlainFile(_) | Node::Rpf8File(..) | Node::Gta4File(..) => {
            bail!("{within} is not a directory")
        }
        Node::Gta4Dir(..) => bail!("{within}: this command does not support GTA IV archives yet"),
        Node::Rpf8Dir(..) => bail!("{within}: for Red Dead Redemption 2 archives use `openrage rpf8` for now"),
        Node::Dir(dir) => {
            let mut found = Vec::new();
            collect_rpfs(&dir, &mut found);
            found
                .into_iter()
                .filter_map(|p| {
                    let rel = p.strip_prefix(game_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    match Rpf7::open(&p, keys.clone()) {
                        Ok(rpf) => Some((rel, Arc::new(rpf), Rpf7::ROOT)),
                        Err(e) => {
                            eprintln!("warning: {e}");
                            None
                        }
                    }
                })
                .collect()
        }
    })
}

#[derive(Default)]
pub(crate) struct VerifyStats {
    pub(crate) archives: usize,
    pub(crate) files: usize,
    pub(crate) resources: usize,
    pub(crate) bytes: u64,
    pub(crate) errors: Vec<String>,
}

pub(crate) fn cmd_verify(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let stats: Vec<VerifyStats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut s = VerifyStats { archives: 1, ..Default::default() };
            let mut nested_errors = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let e = a.entry(id);
                    let result = if e.is_dir() {
                        return;
                    } else if e.is_archive() {
                        s.archives += 1;
                        return;
                    } else if e.resource_version().is_some() {
                        s.resources += 1;
                        a.read_resource_pages(id)
                    } else {
                        s.files += 1;
                        a.read(id)
                    };
                    match result {
                        Ok(data) => s.bytes += data.len() as u64,
                        Err(err) => s.errors.push(format!("{p}: {err}")),
                    }
                },
                &mut |e| nested_errors.push(e.to_string()),
            );
            s.errors.extend(nested_errors);
            eprintln!("  checked {prefix}");
            s
        })
        .collect();

    let mut total = VerifyStats::default();
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
