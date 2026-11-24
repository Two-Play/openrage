//! `openrage verify`: archive data and game-specific formats.

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
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| e.resource_version().is_some(),
        |a, id, p, s: &mut Stats| {
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
                    Ok(openrage_app::textures::ModelCheck::Unreadable(why)) => s.unreadable.push(format!("{p}: {why}")),
                    Err(e) => s.errors.push(format!("{p}: model textures: {e:#}")),
                }
                return;
            }
            if !a.entry(id).name.to_ascii_lowercase().ends_with(".ytd") || a.entry(id).resource_version().is_none() {
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
                            rage_formats::ytd::Layout::Enhanced | rage_formats::ytd::Layout::Rdr2 => &[(0x28, 0x40)],
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
    );
    let mut total = Stats { errors: nested, ..Default::default() };
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
    total.errors.sort();
    total.unreadable.sort();
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
    let (all, nested): (Vec<(Counts, Vec<String>)>, _) = for_each_entry_parallel(
        &starts,
        |e| media::is_media(&e.name.to_ascii_lowercase()),
        |a, id, p, (c, errors): &mut (Counts, Vec<String>)| {
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
    );
    let mut t: Counts = (0, 0, 0.0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3);
    }
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.1).chain(&nested).collect();
    errors.sort();
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
    let (all, nested): (Vec<(Counts, Vec<String>)>, _) = for_each_entry_parallel(
        &starts,
        |e| e.name.to_ascii_lowercase().ends_with(".ysc"),
        |a, id, p, (c, errors): &mut (Counts, Vec<String>)| {
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
                    None => errors.push(format!(
                        "{p}: no opcode table fits ({})",
                        s.check().err().map(|e| e.to_string()).unwrap_or_default()
                    )),
                },
                Err(e) => errors.push(format!("{p}: {e:#}")),
            }
        },
    );
    let mut t: Counts = (0, 0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4);
    }
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.1).chain(&nested).collect();
    errors.sort();
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
    let (all, nested): (Vec<(Counts, Vec<String>)>, _) = for_each_entry_parallel(
        &starts,
        |e| paths::is_path_file(&e.name.to_ascii_lowercase()),
        |a, id, p, (c, errors): &mut (Counts, Vec<String>)| {
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
    );
    let mut t: Counts = (0, 0, 0, 0, 0.0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4, t.5 + c.5);
    }
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.1).chain(&nested).collect();
    errors.sort();
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

/// The triangles of a collision as a mesh to write: every triangle, also
/// of spheres, boxes and capsules (as the viewer builds them), with its
/// material type.
pub(crate) fn collision_mesh(d: &rage_formats::drawable::Drawable) -> rage_formats::ybn_write::CollisionMesh {
    let mut out = rage_formats::ybn_write::CollisionMesh::default();
    for m in &d.meshes {
        let material = d
            .shaders
            .get(m.shader as usize)
            .map_or(0, |s| s.name_hash.wrapping_sub(rage_formats::ybn::MATERIAL_BASE) as u8);
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&m.positions);
        out.indices.extend(m.indices.iter().map(|i| i + base));
        out.materials.extend(std::iter::repeat_n(Some(material), m.indices.len() / 3));
    }
    out
}

/// Checks the game's collision hierarchies against the writer's reading of
/// them, then writes every Enhanced `.ybn` and drawable collision anew from
/// its own triangles (`ybn_write`) and compares what the reader gets back.
pub(crate) fn cmd_verify_collision_writer(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::{drawable, ybn, ybn_write};
    let starts = archive_roots(game_dir, within, keys.clone())?;
    let started = Instant::now();
    // Per material type: triangles and area.
    // Triangles that lie on the coarser vertex grid of the one new bound can collapse
    // (slivers), so triangles may get fewer, never more; the area stays.
    let totals = |d: &drawable::Drawable| -> std::collections::BTreeMap<u32, (usize, f64)> {
        let mut t = std::collections::BTreeMap::new();
        for m in &d.meshes {
            let e = t.entry(d.shaders[m.shader as usize].name_hash).or_insert((0usize, 0f64));
            for c in m.indices.as_chunks::<3>().0 {
                let [a, b, c] = c.map(|i| m.positions[i as usize]);
                let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
                let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                let area = 0.5 * ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]) as f64).sqrt();
                e.0 += 1;
                e.1 += area;
            }
        }
        t
    };
    // (game hierarchies checked, written anew, skipped: no triangle mesh to copy / too large)
    // Collisions of boxes and spheres only borrow the structure of a triangle mesh.
    let donor = openrage_app::obj::find_collision_donor(game_dir, within, keys.clone());
    let donor = donor.as_ref().map(|(r, y)| (r, *y));
    let check = |res: &rage_resource::Resource, ydr: bool, s: &mut (usize, usize, usize)| -> Result<()> {
        let root = ybn_write::collision_of(res, ydr)?;
        for b in ybn_write::bvh_bounds(res, root) {
            ybn_write::check_bvh(res, b).context("the game's own hierarchy")?;
            s.0 += 1;
        }
        let (before, _) = ybn::parse_at(res, root)?;
        let mesh = collision_mesh(&before);
        if mesh.indices.len() / 3 > ybn_write::MAX_TRIANGLES || mesh.indices.is_empty() {
            s.2 += 1;
            return Ok(());
        }
        let file = ybn_write::replace_collision(res, ydr, &mesh, donor)?;
        let back = rage_resource::Resource::from_rsc7(&file)?;
        let at = ybn_write::collision_of(&back, ydr)?;
        for b in ybn_write::bvh_bounds(&back, at) {
            ybn_write::check_bvh(&back, b).context("the written hierarchy")?;
        }
        let (after, _) = ybn::parse_at(&back, at)?;
        let (x, y) = (totals(&before), totals(&after));
        if x.len() != y.len() {
            bail!("materials {:?} written as {:?}", x.keys().collect::<Vec<_>>(), y.keys().collect::<Vec<_>>());
        }
        for ((m, a), (n, b)) in x.iter().zip(&y) {
            if m != n || b.0 > a.0 || (a.1 - b.1).abs() > 0.01 * a.1 + 0.05 {
                bail!(
                    "material {m:x}: {} triangles, area {:.2} written as {} triangles, area {:.2}",
                    a.0,
                    a.1,
                    b.0,
                    b.1
                );
            }
        }
        let slack: Vec<f32> = (0..3).map(|k| (before.bounds_max[k] - before.bounds_min[k]) / 30000.0 + 1e-3).collect();
        if (0..3).any(|k| {
            (before.bounds_min[k] - after.bounds_min[k]).abs() > slack[k]
                || (before.bounds_max[k] - after.bounds_max[k]).abs() > slack[k]
        }) {
            bail!(
                "box {:?}..{:?} written as {:?}..{:?}",
                before.bounds_min,
                before.bounds_max,
                after.bounds_min,
                after.bounds_max
            );
        }
        if ydr {
            let (a, b) = (drawable::parse_ydr(res)?, drawable::parse_ydr(&back)?);
            if a.meshes.len() != b.meshes.len() {
                bail!("the drawable's meshes changed");
            }
        }
        s.1 += 1;
        Ok(())
    };
    type Counts = (usize, (usize, usize, usize), Vec<String>);
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| {
            let name = e.name.to_ascii_lowercase();
            name.ends_with(".ydr") || name.ends_with(".ybn")
        },
        |a, id, p, (files, s, errors): &mut Counts| {
            let ydr = a.entry(id).name.to_ascii_lowercase().ends_with(".ydr");
            let Ok(res) = a.read_resource(id) else { return };
            if ydr && (res.version != drawable::VERSION_YDR_ENHANCED || ybn_write::collision_of(&res, true).is_err()) {
                return;
            }
            *files += 1;
            if let Err(e) = check(&res, ydr, s) {
                errors.push(format!("{p}: {e:#}"));
            }
        },
    );
    let sum = |f: fn(&Counts) -> usize| all.iter().map(f).sum::<usize>();
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.2).chain(&nested).collect();
    errors.sort();
    outln!(
        "{} collisions ({} game hierarchies as expected), {} written anew and read back the same, {} skipped (no triangles, or over {}), {} errors ({:.1}s)",
        sum(|x| x.0),
        sum(|x| x.1.0),
        sum(|x| x.1.1),
        sum(|x| x.1.2),
        rage_formats::ybn_write::MAX_TRIANGLES,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    for e in errors.iter().take(30) {
        outln!("  {e}");
    }
    if errors.is_empty() { Ok(()) } else { bail!("{} collisions failed", errors.len()) }
}

pub(crate) fn cmd_verify_collisions(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize);
    let (all, nested): (Vec<(Counts, Vec<String>)>, _) = for_each_entry_parallel(
        &starts,
        |e| e.name.to_ascii_lowercase().ends_with(".ybn"),
        |a, id, p, (c, errors): &mut (Counts, Vec<String>)| {
            if !a.entry(id).name.to_ascii_lowercase().ends_with(".ybn") {
                return;
            }
            c.0 += 1;
            match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(rage_formats::ybn::parse(&r)?)) {
                Ok((_, s)) => {
                    c.1 += s.bounds;
                    c.2 += s.triangles;
                    c.3 += s.primitives;
                }
                Err(e) => errors.push(format!("{p}: {e:#}")),
            }
        },
    );
    let mut t: Counts = (0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3);
    }
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.1).chain(&nested).collect();
    errors.sort();
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
    let (all, nested): (Vec<(Counts, Vec<String>)>, _) = for_each_entry_parallel(
        &starts,
        |e| e.name.to_ascii_lowercase().ends_with(".ypt"),
        |a, id, p, (c, errors): &mut (Counts, Vec<String>)| {
            if !a.entry(id).name.to_ascii_lowercase().ends_with(".ypt") {
                return;
            }
            c.0 += 1;
            match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(rage_formats::ypt::parse(&r)?)) {
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
    );
    let mut t: Counts = (0, 0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4);
    }
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.1).chain(&nested).collect();
    errors.sort();
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
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| Kind::of(&e.name).is_some(),
        |a, id, p, (files, entries, errors): &mut (usize, usize, Vec<String>)| {
            let Some(kind) = Kind::of(&a.entry(id).name) else { return };
            *files += 1;
            let result = a.read(id).map_err(anyhow::Error::from).and_then(|data| {
                let (text, n) = texttable::to_text(kind, &data)?;
                let back = texttable::from_text(kind, &text)?;
                if back != data {
                    bail!("converting back changes the file ({} → {} bytes)", data.len(), back.len());
                }
                Ok(n)
            });
            match result {
                Ok(n) => *entries += n,
                Err(e) => errors.push(format!("{p}: {e:#}")),
            }
        },
    );
    let (files, entries): (usize, usize) = all.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.2).chain(&nested).collect();
    errors.sort();
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

/// Writes every Enhanced `.ydr`, `.yft` (the body and the children's own
/// models: wheels) and `.ydd` (every drawable) anew with its own meshes
/// (through the model writer) and compares what the reader gets back.
/// Skinned meshes keep the bones and weights of every vertex.
pub(crate) fn cmd_verify_model_writer(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::{drawable, ydr_write};
    use ydr_write::Place;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    // The bone with the most weight, per vertex.
    let main_bones = |m: &drawable::Mesh| -> Vec<u16> {
        m.bone_indices
            .iter()
            .zip(&m.bone_weights)
            .map(|(b, w)| b[(0..4).fold(0, |k, i| if w[i] > w[k] { i } else { k })])
            .collect()
    };
    let single_bone = |m: &drawable::Mesh| m.bone_weights.iter().all(|w| w.iter().filter(|&&x| x > 0.0).count() == 1);
    let byte = |w: f32| (w * 255.0).round() as u8;
    // Places written (0: nothing to write) and child models / dictionary drawables among them.
    let check = |res: &rage_resource::Resource, name: &str| -> Result<(usize, usize, usize)> {
        if res.version != drawable::VERSION_YDR_ENHANCED && res.version != drawable::VERSION_YFT_ENHANCED {
            return Ok((0, 0, 0));
        }
        let places: Vec<Place> = if name.ends_with(".ydd") {
            (0..ydr_write::entry_hashes(res)?.len()).map(Place::Entry).collect()
        } else {
            std::iter::once(Place::Main).chain((0..ydr_write::child_models(res).len()).map(Place::Child)).collect()
        };
        let mut chosen: Vec<(Place, Vec<ydr_write::NewMesh>)> = Vec::new();
        for place in places {
            let before = ydr_write::read_at(res, place)?;
            let usable = ydr_write::usable_shaders_at(res, place).with_context(|| format!("{place:?}"))?;
            let skinned = ydr_write::is_skinned_at(res, place)?;
            let meshes: Vec<ydr_write::NewMesh> = before
                .meshes
                .iter()
                .filter(|m| usable.contains(&m.shader) && !m.indices.is_empty() && m.bone_indices.is_empty() != skinned)
                .map(|m| {
                    // Vertices on one bone each (vehicles) as the OBJ import gives them;
                    // blended ones (peds) with all their bones and weights.
                    let (bones, blend) = if m.bone_indices.is_empty() {
                        (Vec::new(), Vec::new())
                    } else if single_bone(m) {
                        (main_bones(m), Vec::new())
                    } else {
                        let blend = m
                            .bone_indices
                            .iter()
                            .zip(&m.bone_weights)
                            .map(|(b, w)| ydr_write::Blend { bones: *b, weights: *w })
                            .collect();
                        (Vec::new(), blend)
                    };
                    ydr_write::NewMesh {
                        shader: m.shader,
                        positions: m.positions.clone(),
                        normals: m.normals.clone(),
                        uvs: m.uvs.clone(),
                        colors: m.colors.clone(),
                        indices: m.indices.clone(),
                        bones,
                        blend,
                    }
                })
                .collect();
            if !meshes.is_empty() {
                chosen.push((place, meshes));
            }
        }
        if chosen.is_empty() {
            return Ok((0, 0, 0));
        }
        let lods: Vec<[&[ydr_write::NewMesh]; 1]> = chosen.iter().map(|(_, m)| [m.as_slice()]).collect();
        let parts: Vec<ydr_write::Part> = chosen
            .iter()
            .zip(&lods)
            .map(|((place, _), l)| ydr_write::Part { place: *place, lods: l, materials: None })
            .collect();
        let file = ydr_write::replace_parts(res, &parts, None)?;
        let after_res = rage_resource::Resource::from_rsc7(&file)?;
        // What the game gets reads back as a whole.
        if name.ends_with(".ydd") {
            drawable::parse_ydd(&after_res)?;
        } else {
            ydr_write::read_back(&file)?;
        }
        for (place, meshes) in &chosen {
            let after = ydr_write::read_at(&after_res, *place)?;
            if after.meshes.len() != meshes.len() {
                bail!("{place:?}: {} meshes written, {} read back", meshes.len(), after.meshes.len());
            }
            for (i, (a, m)) in after.meshes.iter().zip(meshes).enumerate() {
                // Bit for bit: some of the game's vertices hold NaN, which never equals itself.
                let same = |x: &[f32], y: &[f32]| {
                    x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits())
                };
                if !same(a.positions.as_flattened(), m.positions.as_flattened())
                    || a.indices != m.indices
                    || a.shader != m.shader
                {
                    bail!("{place:?} mesh {i}: geometry differs after writing");
                }
                if !m.uvs.is_empty() && !same(a.uvs.as_flattened(), m.uvs.as_flattened()) {
                    bail!("{place:?} mesh {i}: texture coordinates differ after writing");
                }
                let close = |x: &[[f32; 3]], y: &[[f32; 3]]| {
                    x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (0..3).all(|k| (p[k] - q[k]).abs() < 0.01))
                };
                if !m.normals.is_empty() && !close(&a.normals, &m.normals) {
                    bail!("{place:?} mesh {i}: normals differ after writing");
                }
                if !m.colors.is_empty() && a.colors.first() != m.colors.first() {
                    bail!("{place:?} mesh {i}: colours differ after writing");
                }
                if m.blend.is_empty() {
                    if main_bones(a) != m.bones {
                        bail!("{place:?} mesh {i}: vertices move with other bones after writing");
                    }
                } else {
                    let got = a.bone_indices.iter().zip(&a.bone_weights).map(|(b, w)| (*b, w.map(byte)));
                    if got.ne(m.blend.iter().map(|bl| (bl.bones, bl.weights.map(byte)))) {
                        bail!("{place:?} mesh {i}: bones or weights differ after writing");
                    }
                }
            }
        }
        let children = chosen.iter().filter(|(p, _)| matches!(p, Place::Child(_))).count();
        let entries = chosen.iter().filter(|(p, _)| matches!(p, Place::Entry(_))).count();
        let written = (chosen.len(), children, entries);

        // Materials: a shader copy per mesh shader, with a texture where the
        // place's shader group has an embedded dictionary to put it in.
        // (A model with an empty embedded dictionary borrows the structure from another one when
        // importing; here only models with embedded textures get one.)
        let owner = |p: Place| if matches!(p, Place::Child(_)) { Place::Main } else { p };
        let mut materials: Vec<Vec<ydr_write::NewMaterial>> = Vec::new();
        let mut with_materials = Vec::new();
        for (place, meshes) in &chosen {
            let embedded = ydr_write::read_at(res, owner(*place))?.textures.is_some_and(|t| !t.textures.is_empty());
            let mut mats: Vec<ydr_write::NewMaterial> = Vec::new();
            let mut meshes = meshes.clone();
            for m in &mut meshes {
                let k = match mats.iter().position(|mat| mat.shader == m.shader) {
                    Some(k) => k,
                    None => {
                        let texture = embedded
                            .then(|| {
                                ydr_write::texture_from_rgba(
                                    &format!("openrage_check_{}", mats.len()),
                                    2,
                                    2,
                                    &[200; 16],
                                )
                            })
                            .transpose()?;
                        mats.push(ydr_write::NewMaterial { shader: m.shader, texture });
                        mats.len() - 1
                    }
                };
                m.shader = k as u16;
            }
            materials.push(mats);
            with_materials.push(meshes);
        }
        let lods: Vec<[&[ydr_write::NewMesh]; 1]> = with_materials.iter().map(|m| [m.as_slice()]).collect();
        let parts: Vec<ydr_write::Part> = chosen
            .iter()
            .zip(&lods)
            .zip(&materials)
            .map(|(((place, _), l), mats)| ydr_write::Part { place: *place, lods: l, materials: Some(mats) })
            .collect();
        let file = match ydr_write::replace_parts(res, &parts, None) {
            Ok(f) => f,
            // Shaders without a colour slot cannot take a texture: fine, skip that part.
            Err(e) if e.to_string().contains("colour") => return Ok(written),
            Err(e) => return Err(e.into()),
        };
        let after_res = rage_resource::Resource::from_rsc7(&file)?;
        // Where each part's copies start: after the group's shaders and the copies of earlier parts.
        let mut next: Vec<(Place, usize)> = Vec::new();
        for (((place, _), mats), meshes) in chosen.iter().zip(&materials).zip(&with_materials) {
            let o = owner(*place);
            let first = match next.iter_mut().find(|(p, _)| *p == o) {
                Some((_, n)) => n,
                None => {
                    next.push((o, ydr_write::read_at(res, o)?.shaders.len()));
                    &mut next.last_mut().expect("pushed").1
                }
            };
            let start = *first;
            *first += mats.len();
            let group = ydr_write::read_at(&after_res, o)?;
            for (k, mat) in mats.iter().enumerate() {
                let Some(shader) = group.shaders.get(start + k) else {
                    bail!("{place:?} materials: {} shaders, copy {} missing", group.shaders.len(), start + k);
                };
                if let Some(t) = &mat.texture {
                    if !shader.textures.contains(&t.name) {
                        bail!("{place:?} material {k}: its shader does not use {}", t.name);
                    }
                    let found = group.textures.as_ref().and_then(|d| d.textures.iter().find(|x| x.name == t.name));
                    if found.and_then(|x| x.to_rgba()).as_deref() != Some(&[200u8; 16][..]) {
                        bail!("{place:?} material {k}: texture {} not embedded intact", t.name);
                    }
                }
            }
            let after = ydr_write::read_at(&after_res, *place)?;
            if after.meshes.iter().map(|m| m.shader as usize).ne(meshes.iter().map(|m| start + m.shader as usize)) {
                bail!("{place:?} materials: meshes do not use their shader copies");
            }
        }
        for (o, n) in &next {
            let got = ydr_write::read_at(&after_res, *o)?.shaders.len();
            if got != *n {
                bail!("{o:?} materials: {got} shaders, expected {n}");
            }
        }
        Ok(written)
    };
    type Counts = (usize, usize, usize, usize, Vec<String>);
    let is_model = |name: &str| name.ends_with(".ydr") || name.ends_with(".yft") || name.ends_with(".ydd");
    let (results, nested) = for_each_entry_parallel(
        &starts,
        |e| is_model(&e.name.to_ascii_lowercase()),
        |a, id, p, (seen, written, children, entries, errors): &mut Counts| {
            let name = a.entry(id).name.to_ascii_lowercase();
            *seen += 1;
            match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| check(&r, &name)) {
                Ok((0, _, _)) => {}
                Ok((_, c, e)) => {
                    *written += 1;
                    *children += c;
                    *entries += e;
                }
                Err(e) => errors.push(format!("{p}: {e:#}")),
            }
        },
    );
    let sum = |f: fn(&Counts) -> usize| results.iter().map(f).sum::<usize>();
    let mut errors: Vec<&String> = results.iter().flat_map(|r| &r.4).chain(&nested).collect();
    errors.sort();
    outln!(
        "{} models, {} written anew and read back identically ({} fragment child models, {} dictionary drawables among them), {} errors ({:.1}s)",
        sum(|r| r.0),
        sum(|r| r.1),
        sum(|r| r.2),
        sum(|r| r.3),
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    for e in errors.iter().take(30) {
        outln!("  {e}");
    }
    if errors.is_empty() { Ok(()) } else { bail!("{} models failed", errors.len()) }
}

pub(crate) fn cmd_verify_meta(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::metadata;
    use std::collections::BTreeMap;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = BTreeMap<String, (usize, usize)>;
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| metadata::is_metadata(&e.name),
        |a, id, p, (counts, errors, problems): &mut (Counts, Vec<String>, Vec<String>)| {
            let name = &a.entry(id).name;
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
    );
    let mut counts = BTreeMap::<String, (usize, usize)>::new();
    for (c, _, _) in &all {
        for (k, v) in c {
            let e = counts.entry(k.clone()).or_default();
            e.0 += v.0;
            e.1 += v.1;
        }
    }
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.1).chain(&nested).collect();
    errors.sort();
    let mut problems: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    problems.sort();
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

/// What converting one PSO file to XML and back gave.
pub(crate) enum PsoRoundTrip {
    /// The same bytes.
    Identical,
    /// Other bytes that read back to the same XML.
    SameXml,
    /// The original has parts that cannot be read, so its XML is incomplete.
    Unreadable,
}

/// Converts a PSO file to XML and back, and compares.
pub(crate) fn pso_round_trip(data: &[u8], rdr2: bool) -> Result<PsoRoundTrip, String> {
    use rage_formats::pso;
    let parse = |d: &[u8]| if rdr2 { pso::parse_rdr2(d) } else { pso::parse(d) };
    let original = parse(data).map_err(|e| e.to_string())?;
    let (xml, problems) = original.to_xml_with_limit(1 << 30);
    if !problems.is_empty() {
        return Ok(PsoRoundTrip::Unreadable);
    }
    let written = original.from_xml(&xml).map_err(|e| format!("writing: {e}"))?;
    if written == data {
        return Ok(PsoRoundTrip::Identical);
    }
    let (again, problems) = parse(&written).map_err(|e| format!("reading back: {e}"))?.to_xml_with_limit(1 << 30);
    if let Some(p) = problems.first() {
        return Err(format!("reading back: {p}"));
    }
    if again != xml {
        let line = xml.lines().zip(again.lines()).position(|(a, b)| a != b).unwrap_or(0);
        let show = |t: &str| t.lines().nth(line).unwrap_or("(end)").trim().chars().take(120).collect::<String>();
        return Err(format!("XML differs at line {}: {} → {}", line + 1, show(&xml), show(&again)));
    }
    Ok(PsoRoundTrip::SameXml)
}

pub(crate) fn cmd_verify_pso_rebuild(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    // Per archive: identical, same XML, unreadable, errors.
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| {
            !(e.is_dir() || e.is_archive() || e.resource_version().is_some())
                && openrage_app::metadata::is_metadata(&e.name)
        },
        |a, id, p, (same, xml, unreadable, errors): &mut (usize, usize, usize, Vec<String>)| {
            let data = match a.read(id) {
                Ok(d) => d,
                Err(err) => return errors.push(format!("{p}: {err}")),
            };
            if !rage_formats::pso::is_pso(&data) {
                return;
            }
            match pso_round_trip(&data, false) {
                Ok(PsoRoundTrip::Identical) => *same += 1,
                Ok(PsoRoundTrip::SameXml) => *xml += 1,
                Ok(PsoRoundTrip::Unreadable) => *unreadable += 1,
                Err(err) => errors.push(format!("{p}: {err}")),
            }
        },
    );
    let same: usize = all.iter().map(|x| x.0).sum();
    let xml: usize = all.iter().map(|x| x.1).sum();
    let unreadable: usize = all.iter().map(|x| x.2).sum();
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.3).chain(&nested).collect();
    errors.sort();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} PSO files: {same} identical, {xml} with other bytes but the same XML, {unreadable} not fully readable \
         (skipped), {} errors ({:.1}s)",
        same + xml + unreadable + errors.len(),
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} PSO files do not convert back", errors.len());
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
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| e.name.to_ascii_lowercase().ends_with(".ycd") && e.resource_version().is_some(),
        |a, id, p, (files, clips, anims, mismatch, badq, errors): &mut AnimStats| {
            *files += 1;
            let parsed = a
                .read_resource(id)
                .map_err(|e| e.to_string())
                .and_then(|r| rage_formats::clip::parse_ycd(&r).map_err(|e| e.to_string()));
            match parsed {
                Ok(d) => {
                    *clips += d.clips.len();
                    *anims += d.animations.len();
                    for an in &d.animations {
                        let blocks = an.block_frames();
                        let sum: usize = blocks.iter().sum();
                        let overlapped = sum.saturating_sub(blocks.len().saturating_sub(1));
                        if sum != an.frames && overlapped != an.frames {
                            *mismatch += 1;
                        }
                        for (t, tr) in an.tracks.iter().enumerate() {
                            for f in (0..an.frames).step_by(7) {
                                match an.value(t, f as f32) {
                                    Some(v) if tr.format == 1 => {
                                        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                                        if !(0.99..=1.01).contains(&n) {
                                            *badq += 1;
                                        }
                                    }
                                    Some(v) if v.iter().any(|x| !x.is_finite()) => *badq += 1,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(e) => errors.push(format!("{p}: {e}")),
            }
        },
    );
    let sum = |f: fn(&AnimStats) -> usize| all.iter().map(f).sum::<usize>();
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.5).chain(&nested).collect();
    errors.sort();
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
    let (all, nested) =
        for_each_entry_parallel(
            &starts,
            |e| openrage_app::audio::is_audio(&e.name),
            |a,
             id,
             p,
             (files, sounds, secs, encrypted, not_audio, errors): &mut (
                usize,
                usize,
                f64,
                usize,
                usize,
                Vec<String>,
            )| {
                *files += 1;
                let result = a
                    .read(id)
                    .map_err(anyhow::Error::from)
                    .and_then(|d| openrage_app::audio::parse(d, keys.as_deref()))
                    .and_then(|awc| {
                        if awc.encrypted || awc.flags & 0xA != 0 {
                            *encrypted += 1;
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
                        *sounds += pcms.len();
                        *secs += pcms.iter().map(|p| p.duration_secs() as f64).sum::<f64>();
                    }
                    // A few files only share the extension (e.g. shader caches).
                    Err(e)
                        if matches!(
                            e.downcast_ref::<rage_formats::awc::AwcError>(),
                            Some(rage_formats::awc::AwcError::NotAwc)
                        ) =>
                    {
                        *files -= 1;
                        *not_audio += 1;
                    }
                    Err(e) => errors.push(format!("{p}: {e:#}")),
                }
            },
        );
    let files: usize = all.iter().map(|x| x.0).sum();
    let sounds: usize = all.iter().map(|x| x.1).sum();
    let secs: f64 = all.iter().map(|x| x.2).sum();
    let encrypted: usize = all.iter().map(|x| x.3).sum();
    let not_audio: usize = all.iter().map(|x| x.4).sum();
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.5).chain(&nested).collect();
    errors.sort();
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
    let is_model = |n: &str| n.ends_with(".ydr") || n.ends_with(".yft") || n.ends_with(".ydd");
    let (all, nested) = for_each_entry_parallel(
        &starts,
        |e| is_model(&e.name.to_ascii_lowercase()) && e.resource_version().is_some(),
        |a, id, p, (models, tris, errors): &mut (usize, usize, Vec<String>)| {
            let n = a.entry(id).name.to_ascii_lowercase();
            *models += 1;
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
                Ok(ds) => *tris += ds.iter().map(|d| d.triangle_count()).sum::<usize>(),
                Err(e) => errors.push(format!("{p}: {e}")),
            }
        },
    );
    let (models, tris): (usize, usize) = all.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let mut errors: Vec<&String> = all.iter().flat_map(|x| &x.2).chain(&nested).collect();
    errors.sort();
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

/// What a worker gets: the archive the file is in (a nested one too), the file, its path, and the
/// archive root it was found under (to report that root done once all its files are checked).
type Job<A, I> = (Arc<A>, I, String, Arc<Pending>);

/// Files of one archive root still queued or being checked, plus one while it is walked.
struct Pending {
    left: std::sync::atomic::AtomicUsize,
    prefix: String,
    report: bool,
}

impl Pending {
    fn done(&self) {
        if self.left.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 && self.report {
            eprintln!("  checked {}", self.prefix);
        }
    }
}

/// Runs `work` on every file `walk` finds below the archive roots, on one
/// thread per core, each with its own stats (returned unmerged), plus the
/// errors of walking (nested archives that do not open).
///
/// Roots are walked in parallel (rayon), and the files go through a short
/// queue to the workers: one big archive (all vehicles are in one) does not
/// leave the other cores idle. Reading and decompressing happen in the
/// workers too. The workers are plain threads, not rayon tasks: walkers wait
/// on a full queue, and waiting rayon tasks could starve rayon workers.
fn run_parallel<A, I, S>(
    starts: &[(String, Arc<A>, I)],
    report: bool,
    walk: impl Fn(&Arc<A>, I, &str, &mut dyn FnMut(&Arc<A>, I, &str), &mut Vec<String>) + Sync,
    work: impl Fn(&A, I, &str, &mut S) + Sync,
) -> (Vec<S>, Vec<String>)
where
    A: Send + Sync,
    I: Copy + Send + Sync,
    S: Default + Send,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let (send, receive) = std::sync::mpsc::sync_channel::<Job<A, I>>(workers * 4);
    // Owned by the workers only: should they all stop (a panic), the walkers stop waiting.
    let receive = Arc::new(std::sync::Mutex::new(receive));
    let work = &work;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let receive = Arc::clone(&receive);
                scope.spawn(move || {
                    let mut s = S::default();
                    loop {
                        // The lock is held only while waiting for the next file.
                        let job = receive.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok((archive, id, path, pending)) = job else { break };
                        work(&archive, id, &path, &mut s);
                        pending.done();
                    }
                    s
                })
            })
            .collect();
        drop(receive);
        let errors: Vec<String> = starts
            .par_iter()
            .flat_map_iter(|(prefix, archive, dir)| {
                let pending = Arc::new(Pending { left: AtomicUsize::new(1), prefix: prefix.clone(), report });
                let mut errors = Vec::new();
                let send = send.clone();
                walk(
                    archive,
                    *dir,
                    prefix,
                    &mut |a, id, p| {
                        pending.left.fetch_add(1, Ordering::AcqRel);
                        if send.send((Arc::clone(a), id, p.to_owned(), Arc::clone(&pending))).is_err() {
                            pending.done();
                        }
                    },
                    &mut errors,
                );
                pending.done();
                errors
            })
            .collect();
        drop(send);
        let stats = handles.into_iter().map(|h| h.join().expect("verify worker")).collect();
        (stats, errors)
    })
}

/// Runs `work` on every entry below the archive roots (nested archives too)
/// that `wanted` accepts, on every core, also within one archive.
/// Returns the stats of every worker, unmerged, and the errors of nested
/// archives that do not open.
pub(crate) fn for_each_entry_parallel<S: Default + Send>(
    starts: &[Start],
    wanted: impl Fn(&rage_rpf::Entry) -> bool + Sync,
    work: impl Fn(&Rpf7, EntryId, &str, &mut S) + Sync,
) -> (Vec<S>, Vec<String>) {
    for_each_entry_parallel_reporting(starts, false, wanted, work)
}

/// [`for_each_entry_parallel`], printing `checked <root>` once all files of a root are checked.
pub(crate) fn for_each_entry_parallel_reporting<S: Default + Send>(
    starts: &[Start],
    report: bool,
    wanted: impl Fn(&rage_rpf::Entry) -> bool + Sync,
    work: impl Fn(&Rpf7, EntryId, &str, &mut S) + Sync,
) -> (Vec<S>, Vec<String>) {
    // Like `vfs::walk`, but every archive shared, so workers can read from it.
    fn walk(
        archive: &Arc<Rpf7>,
        dir: EntryId,
        prefix: &str,
        wanted: &(dyn Fn(&rage_rpf::Entry) -> bool + Sync),
        emit: &mut dyn FnMut(&Arc<Rpf7>, EntryId, &str),
        errors: &mut Vec<String>,
    ) {
        for child in archive.children(dir) {
            let entry = archive.entry(child);
            let path = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
            if wanted(entry) {
                emit(archive, child, &path);
            }
            if entry.is_dir() {
                walk(archive, child, &path, wanted, emit, errors);
            } else if entry.is_archive() {
                match archive.open_child(child) {
                    Ok(inner) => walk(&Arc::new(inner), Rpf7::ROOT, &path, wanted, emit, errors),
                    Err(e) => errors.push(e.to_string()),
                }
            }
        }
    }
    run_parallel(starts, report, |a, dir, prefix, emit, errors| walk(a, dir, prefix, &wanted, emit, errors), work)
}

/// [`for_each_entry_parallel`] for GTA IV archives (`.rpf`, `.img`; not nested).
pub(crate) fn for_each_gta4_entry_parallel<S: Default + Send>(
    starts: &[crate::cmd::verify_gta4::Gta4Start],
    report: bool,
    wanted: impl Fn(&rage_rpf::Gta4Archive, rage_rpf::Gta4EntryId) -> bool + Sync,
    work: impl Fn(&rage_rpf::Gta4Archive, rage_rpf::Gta4EntryId, &str, &mut S) + Sync,
) -> Vec<S> {
    let (stats, _) = run_parallel(
        starts,
        report,
        |a, dir, prefix, emit, _| {
            vfs::walk_gta4(a, dir, prefix, &mut |id, p| {
                if wanted(a, id) {
                    emit(a, id, p)
                }
            })
        },
        work,
    );
    stats
}

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
        _ => bail!("{within}: not supported"),
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
    match vfs::resolve(game_dir, within, keys.clone())? {
        node @ (Node::Rpf9Dir(..) | Node::Rpf9File(..)) => return verify_rpf9(node, within),
        node @ Node::Dir(_) if openrage_app::detect::is_gta6_dir(game_dir) => return verify_rpf9(node, within),
        _ => {}
    }
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let (stats, nested) = for_each_entry_parallel_reporting(
        &starts,
        true,
        |e| !e.is_dir(),
        |a, id, p, s: &mut VerifyStats| {
            let e = a.entry(id);
            let result = if e.is_archive() {
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
    );

    let mut total = VerifyStats { archives: starts.len(), errors: nested, ..Default::default() };
    for s in stats {
        total.archives += s.archives;
        total.files += s.files;
        total.resources += s.resources;
        total.bytes += s.bytes;
        total.errors.extend(s.errors);
    }
    total.errors.sort();
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

/// Checks bytes recovered from RPF9, without claiming to verify its unread
/// table of contents or the files the recovery scan cannot discover.
fn verify_rpf9(node: Node, within: &str) -> Result<()> {
    let started = Instant::now();
    let mut total = VerifyStats::default();
    match node {
        Node::Rpf9Dir(rpf, dir) => verify_rpf9_archive(&rpf, &dir, None, &mut total)?,
        Node::Rpf9File(rpf, i) => verify_rpf9_archive(&rpf, "", Some(i), &mut total)?,
        Node::Dir(dir) => {
            let mut found = Vec::new();
            collect_rpfs(&dir, &mut found);
            for path in found.into_iter().filter(|p| rage_rpf::Rpf9::file_is_rpf9(p)) {
                match rage_rpf::Rpf9::open(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|rpf| verify_rpf9_archive(&rpf, "", None, &mut total))
                {
                    Ok(()) => {}
                    Err(e) => total.errors.push(format!("{}: {e:#}", path.display())),
                }
            }
        }
        _ => bail!("{within}: not an RPF9 recovery path"),
    }
    if total.archives == 0 && total.errors.is_empty() {
        bail!("{within}: no RPF9 archives found");
    }
    for error in &total.errors {
        outln!("FAIL {error}");
    }
    outln!(
        "{} RPF9 archives, {} recovered files checked, {} decoded, {} errors ({:.1}s)",
        total.archives,
        total.files,
        human(total.bytes),
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    outln!(
        "Recovery verification only: zlib decoding, entry bounds and unpacked sizes. Original names, encrypted table of contents and undiscovered data are not verified; raw audio/video checks do not validate their media contents."
    );
    if !total.errors.is_empty() {
        bail!("verification found {} errors", total.errors.len());
    }
    Ok(())
}

fn verify_rpf9_archive(rpf: &rage_rpf::Rpf9, dir: &str, file: Option<usize>, total: &mut VerifyStats) -> Result<()> {
    let entries = rpf.entries()?;
    total.archives += 1;
    let coverage = openrage_app::rpf9::coverage(rpf, &entries);
    outln!(
        "{}: recovery coverage: {} files, {} stored of {} data, {} unread (includes padding)",
        rpf.display_name(),
        coverage.files,
        human(coverage.stored),
        human(rpf.data_len()),
        human(coverage.unread)
    );
    if file.is_some_and(|i| i >= entries.len()) {
        bail!("invalid recovery entry index");
    }
    let prefix =
        if dir.is_empty() { String::new() } else { format!("{}/", dir.trim_matches('/').to_ascii_lowercase()) };
    for (i, entry) in entries.iter().enumerate() {
        let name = rpf.entry_name(entry);
        if file.map_or_else(|| !name.to_ascii_lowercase().starts_with(&prefix), |selected| selected != i) {
            continue;
        }
        total.files += 1;
        match rpf.copy_to(entry, &mut std::io::sink()) {
            Ok(bytes) if bytes == entry.size => total.bytes += bytes,
            Ok(bytes) => total.errors.push(format!(
                "{}/{name}: expected {} unpacked bytes, read {bytes}",
                rpf.display_name(),
                entry.size
            )),
            Err(error) => total.errors.push(format!("{}/{name}: {error}", rpf.display_name())),
        }
    }
    Ok(())
}

#[cfg(test)]
mod rpf9_tests {
    use super::*;

    fn archive(name: &str) -> (PathBuf, rage_rpf::Rpf9) {
        let dir = std::env::temp_dir().join(format!("openrage-cli-rpf9-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"9FPR");
        bytes[4..8].copy_from_slice(&64u32.to_le_bytes());
        // A complete zlib stream containing "hello", followed by alignment.
        bytes.extend_from_slice(b"\x78\x9c\xcb\x48\xcd\xc9\xc9\x07\x00\x06\x2c\x02\x15");
        bytes.resize(80, 0);
        let path = dir.join("common.rpf");
        fs::write(&path, bytes).unwrap();
        (dir, rage_rpf::Rpf9::open(&path).unwrap())
    }

    #[test]
    fn verifies_recovered_paths_and_skips_other_archive_formats() {
        let (dir, rpf) = archive("paths");
        fs::write(dir.join(openrage_app::detect::GTA6_EXE), b"MZ").unwrap();
        fs::write(dir.join("other.rpf"), b"not RPF9").unwrap();
        fs::create_dir(dir.join("empty")).unwrap();
        assert!(cmd_verify(&dir, "empty", None).is_err());
        let entries = rpf.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 5);
        let name = rpf.entry_name(&entries[0]);
        assert!(cmd_verify(&dir, "", None).is_ok());
        assert!(cmd_verify(&dir, "common.rpf", None).is_ok());
        assert!(cmd_verify(&dir, "common.rpf/txt", None).is_ok());
        assert!(cmd_verify(&dir, &format!("common.rpf/{name}"), None).is_ok());
        let mut stats = VerifyStats::default();
        verify_rpf9_archive(&rpf, "txt", None, &mut stats).unwrap();
        assert_eq!((stats.archives, stats.files, stats.bytes), (1, 1, 5));
        assert!(stats.errors.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn detects_corrupted_data_against_a_recovery_index() {
        let (dir, rpf) = archive("corrupt");
        let entries = rpf.entries().unwrap();
        assert_eq!(entries.len(), 1);
        // Keep the already-recovered index, then break the stream checksum.
        let mut bytes = fs::read(rpf.path()).unwrap();
        bytes[76] ^= 1;
        fs::write(rpf.path(), bytes).unwrap();
        let mut stats = VerifyStats::default();
        verify_rpf9_archive(&rpf, "", Some(0), &mut stats).unwrap();
        assert_eq!((stats.files, stats.errors.len()), (1, 1));
        assert!(verify_rpf9(Node::Rpf9Dir(Arc::new(rpf), String::new()), "common.rpf").is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
