//! Changing files through the mods folder: textures, models, loose files, history and undo.

use crate::*;

pub(crate) fn cmd_repack(
    game_dir: &Path,
    archive: &str,
    out: &Path,
    sets: &[String],
    removes: &[String],
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use openrage_app::repack::{Edit, repack_into};
    let rpf = match vfs::resolve(game_dir, archive, keys)? {
        Node::ArchiveDir(rpf, id) if id == Rpf7::ROOT => rpf,
        _ => bail!("{archive} is not an archive"),
    };
    let mut edits = std::collections::BTreeMap::new();
    for s in sets {
        let (inner, file) = s.split_once('=').context("--set expects inner/path=local_file")?;
        edits.insert(inner.to_owned(), Edit::Set(fs::read(file).with_context(|| format!("reading {file}"))?));
    }
    for r in removes {
        edits.insert(r.clone(), Edit::Remove);
    }
    let mut file =
        std::io::BufWriter::new(fs::File::create(out).with_context(|| format!("creating {}", out.display()))?);
    let written = repack_into(&rpf, &edits, &mut file)?;
    std::io::Write::flush(&mut file)?;
    outln!("Wrote {} ({}) with {} edits.", out.display(), human(written), edits.len());
    Ok(())
}

pub(crate) fn load_ytd(
    game_dir: &Path,
    path: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<rage_formats::ytd::TextureDictionary> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveFile(rpf, id) => gta5_ytd(&rpf, id, path),
        Node::Rpf8File(rpf, i) => rdr2_ytd(&rpf, i),
        Node::Gta4File(a, id) => gta4_ytd(&a, id, path),
        // An RDR2 dictionary replaced in lml/replace (an RSC8 file).
        Node::PlainFile(p) => rsc8_ytd(&p, path),
        Node::Rpf9File(rpf, i) => openrage_app::textures::gta6_dictionary(&rpf, i),
        _ => bail!("{path} is not a file inside an archive"),
    }
}

fn rdr2_ytd(rpf: &rage_rpf::rpf8::Rpf8, i: usize) -> Result<rage_formats::ytd::TextureDictionary> {
    let entry = &rpf.entries().context("no entries")?[i];
    Ok(rage_formats::ytd::parse_rdr2(&rpf.read_resource(entry)?)?)
}

fn rsc8_ytd(p: &Path, path: &str) -> Result<rage_formats::ytd::TextureDictionary> {
    let res = rage_rpf::rpf8::parse_resource_file(&std::fs::read(p)?)
        .with_context(|| format!("{path} is not a file inside an archive or an RSC8 resource"))?;
    Ok(rage_formats::ytd::parse_rdr2(&res)?)
}

/// A `.ytd`, or the textures of a GTA V model of its own.
fn gta5_ytd(rpf: &Rpf7, id: EntryId, path: &str) -> Result<rage_formats::ytd::TextureDictionary> {
    if openrage_app::textures::gta5_model_ext(path).is_some() {
        return openrage_app::textures::gta5_model_textures(&rpf.read_resource(id)?, path)
            .with_context(|| format!("{path} has no textures of its own"));
    }
    Ok(rage_formats::ytd::parse(&rpf.read_resource(id)?)?)
}

/// A GTA IV texture dictionary, or the textures of a model of its own.
fn gta4_ytd(
    a: &rage_rpf::Gta4Archive,
    id: rage_rpf::Gta4EntryId,
    path: &str,
) -> Result<rage_formats::ytd::TextureDictionary> {
    if openrage_app::textures::is_gta4_model(path) {
        return openrage_app::textures::gta4_model_textures(&a.read_resource(id)?)
            .with_context(|| format!("{path} has no textures of its own"));
    }
    Ok(rage_formats::ytd::parse_gta4(&a.read_resource(id)?)?)
}

pub(crate) fn cmd_textures(
    game_dir: &Path,
    path: &str,
    export: Option<&Path>,
    png: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let dict = load_ytd(game_dir, path, keys)?;
    for t in &dict.textures {
        outln!(
            "{:<32} {:>5}x{:<5} {:<14} mips {:>2}  {:>8}  {}",
            t.name,
            t.width,
            t.height,
            t.format.name(),
            t.levels,
            human(t.data.len() as u64),
            t.usage_name()
        );
    }
    for e in &dict.errors {
        eprintln!("warning: {e}");
    }
    let Some(dir) = export else { return Ok(()) };
    fs::create_dir_all(dir)?;
    let mut written = 0;
    for t in &dict.textures {
        let Some(name) = vfs::safe_relative_path(&t.name) else {
            eprintln!("warning: skipped unsafe texture name {:?}", t.name);
            continue;
        };
        let (target, bytes) = if png {
            let Some(rgba) = t.to_rgba() else {
                eprintln!("warning: {}: no PNG decoder for {}", t.name, t.format.name());
                continue;
            };
            (dir.join(name).with_extension("png"), rage_formats::png(t.width, t.height, &rgba)?)
        } else {
            (dir.join(name).with_extension("dds"), rage_formats::dds::to_dds(t))
        };
        fs::write(&target, bytes).with_context(|| format!("writing {}", target.display()))?;
        written += 1;
    }
    outln!("Exported {written} textures to {}", dir.display());
    Ok(())
}

pub(crate) fn cmd_edit_textures(
    game_dir: &Path,
    path: &str,
    sets: &[String],
    removes: &[String],
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use rage_formats::ytd_write::Edit;
    let mut edits = Vec::new();
    for s in sets {
        let (name, file) = s.split_once('=').context("--set expects name=file.dds")?;
        edits.push(Edit::Set(openrage_app::textures::texture_from_dds(name, Path::new(file))?));
    }
    edits.extend(removes.iter().map(|r| Edit::Remove(r.clone())));
    let started = Instant::now();
    let (result, written) = openrage_app::textures::edit_textures(game_dir, keys, path, &edits)?;
    outln!(
        "{}: {} replaced/added ({} new), {} removed — {}",
        path,
        result.replaced + result.added,
        result.added,
        result.removed,
        match (openrage_app::textures::gta5_model_ext(path).is_some(), result.in_place) {
            (true, true) => "written where the model had it",
            (true, false) => "written into pages added to the model",
            (false, true) => "pixel data replaced in place",
            (false, false) => "dictionary rebuilt",
        }
    );
    outln!("Written to {} ({:.1}s)", written.display(), started.elapsed().as_secs_f32());
    Ok(())
}

pub(crate) fn report(applied: &install::Applied, started: Instant) {
    if applied.created {
        outln!("Copied the archive into the overlay folder first (mods; GTA IV: update; RDR2: lml/replace).");
    }
    outln!(
        "Written to {} ({} appended{}, {:.2}s). Undo with `openrage undo`.",
        applied.path.display(),
        human(applied.stats.appended),
        if applied.stats.relocated > 0 {
            format!(", {} entries relocated", applied.stats.relocated)
        } else {
            String::new()
        },
        started.elapsed().as_secs_f32()
    );
}

pub(crate) fn cmd_edit(
    game_dir: &Path,
    path: &str,
    op: Op,
    description: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let started = Instant::now();
    let in_game = install::write_target(game_dir) == install::WriteTarget::Game;
    if in_game {
        eprintln!("note: writing into the game folder itself (`openrage write-target update` to change that).");
    } else if let Some(note) = openrage_app::gta4::update_folder_note(game_dir) {
        eprintln!("note: {note}");
    }
    // Loose files (not inside an archive) go into the overlay folder.
    if install::split(game_dir, path).is_none() {
        let folder = if in_game { "game" } else { rage_mods::overlay::folder_name(game_dir) };
        match op {
            Op::Set(data) => {
                let written = install::put_loose(game_dir, path, &data, description)?;
                outln!("Written to {} (into the {folder} folder). Undo with `openrage undo`.", written.display());
                return Ok(());
            }
            Op::Remove => {
                install::remove_loose(game_dir, path, description)?;
                outln!("Removed {path} from the {folder} folder. Undo with `openrage undo`.");
                return Ok(());
            }
            _ => bail!("{path} is not inside an archive"),
        }
    }
    let applied = install::apply_at(game_dir, keys, path, op, description)?;
    report(&applied, started);
    Ok(())
}

pub(crate) struct ReplaceOptions<'a> {
    pub(crate) out: Option<&'a Path>,
    pub(crate) mods: bool,
    pub(crate) shader: Option<u16>,
    pub(crate) lods: &'a [PathBuf],
    pub(crate) textures: bool,
    pub(crate) drawable: Option<&'a str>,
    /// An OBJ model to build the collision from (`.ydr`); for a `.ybn` the OBJ itself is.
    pub(crate) collision: Option<&'a Path>,
}

pub(crate) fn cmd_replace_model(
    game_dir: &Path,
    vpath: &str,
    obj: &Path,
    o: ReplaceOptions,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let Node::ArchiveFile(rpf, id) = rage_mods::resolve(game_dir, vpath, keys.clone())?.0 else {
        bail!("{vpath} is not a file inside an archive")
    };
    let lower = vpath.to_ascii_lowercase();
    if lower.ends_with(".ybn") {
        let template = rpf.read_resource(id)?;
        let file = with_collision(game_dir, vpath, &template, false, obj, keys.clone())?;
        return save_model(game_dir, vpath, obj, file, &o, keys);
    }
    check_replace_options(&lower, vpath, &o)?;
    let template = rpf.read_resource(id)?;
    replace_drawable(game_dir, vpath, obj, &o, &template, keys)
}

/// Replaces the meshes of a drawable, fragment or drawable dictionary.
fn replace_drawable(
    game_dir: &Path,
    vpath: &str,
    obj: &Path,
    o: &ReplaceOptions,
    template: &rage_resource::Resource,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use openrage_app::obj;
    let (levels, textures) = read_levels(obj, o)?;
    // Vehicles: parts named like bones (`door_dside_f`, `bonnet`) move with them,
    // parts named after the wheel (`wheel_lf`) replace its model.
    // Drawable dictionaries: parts named after a drawable go to it.
    let options = obj::ImportOptions { shader: o.shader, drawable: o.drawable };
    let imported = obj::import(template, vpath, levels, &textures, options, || {
        obj::find_texture_donor(game_dir, vpath, keys.clone())
    })?;
    let back = report_import(&imported)?;
    let file = imported.file;
    let file = match o.collision {
        Some(col) => with_collision(game_dir, vpath, &back, true, col, keys.clone())?,
        None => file,
    };
    save_model(game_dir, vpath, obj, file, o, keys)
}

/// Most further detail levels (`--lod`) a model takes.
const MAX_FURTHER_LODS: usize = 3;

/// Side of the texture made for a material that only has a colour.
const COLOUR_TEXTURE_SIDE: u32 = 4;

fn check_replace_options(lower: &str, vpath: &str, o: &ReplaceOptions) -> Result<()> {
    if o.collision.is_some() && !lower.ends_with(".ydr") {
        bail!("--collision is for drawables (.ydr); vehicles and other fragments keep theirs");
    }
    if !lower.ends_with(".ydr") && !lower.ends_with(".yft") && !lower.ends_with(".ydd") {
        bail!("{vpath} is not a drawable (.ydr), fragment (.yft) or drawable dictionary (.ydd)");
    }
    if o.lods.len() > MAX_FURTHER_LODS {
        bail!("at most three further detail levels (--lod)");
    }
    Ok(())
}

type Textures = std::collections::HashMap<String, rage_formats::texture::Texture>;

/// Every level's OBJ, and the textures their materials name.
fn read_levels(obj: &Path, o: &ReplaceOptions) -> Result<(Vec<Vec<openrage_app::obj::ObjMesh>>, Textures)> {
    let mut files = vec![obj.to_path_buf()];
    files.extend(o.lods.iter().cloned());
    let mut levels = Vec::new();
    let mut textures = Textures::new();
    for f in &files {
        let text = fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        if o.textures {
            read_material_textures(&text, f, &mut textures)?;
        }
        levels.push(openrage_app::obj::read(&text).with_context(|| format!("reading {}", f.display()))?);
    }
    Ok((levels, textures))
}

fn read_material_textures(text: &str, obj_file: &Path, textures: &mut Textures) -> Result<()> {
    let dir = obj_file.parent().unwrap_or(Path::new("."));
    for (material, mtl) in openrage_app::obj::materials(text, dir) {
        if textures.contains_key(&material) {
            continue;
        }
        if let Some(texture) = material_texture(&material, mtl)? {
            textures.insert(material, texture);
        }
    }
    Ok(())
}

/// The texture of an OBJ material: its picture, or a small one of its colour.
fn material_texture(
    material: &str,
    mtl: openrage_app::obj::MtlMaterial,
) -> Result<Option<rage_formats::texture::Texture>> {
    use openrage_app::obj;
    use rage_formats::ydr_write;
    let texture = match (mtl.texture, mtl.colour) {
        (Some(png), _) => {
            let name = obj::texture_name(&png.file_stem().unwrap_or_default().to_string_lossy());
            let (w, h, rgba) = obj::read_png(&png).with_context(|| format!("texture of material {material}"))?;
            ydr_write::texture_from_rgba(&name, w, h, &rgba)?
        }
        // Only a colour: a small texture of that colour.
        (None, Some(c)) => {
            let px = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            let pixels = (COLOUR_TEXTURE_SIDE * COLOUR_TEXTURE_SIDE) as usize;
            let rgba: Vec<u8> = (0..pixels).flat_map(|_| [px[0], px[1], px[2], 255]).collect();
            let name = format!("{}_colour", obj::texture_name(material));
            ydr_write::texture_from_rgba(&name, COLOUR_TEXTURE_SIDE, COLOUR_TEXTURE_SIDE, &rgba)?
        }
        (None, None) => return Ok(None),
    };
    Ok(Some(texture))
}

/// Reports what the import did; returns the written model read back.
fn report_import(imported: &openrage_app::obj::Imported) -> Result<rage_resource::Resource> {
    let several = imported.targets.len() > 1;
    print_imported(imported, several);
    let back = rage_resource::Resource::from_rsc7(&imported.file)?;
    print_closest_levels(&back, &imported.targets, several)?;
    outln!("{} detail level(s), {}", imported.levels, human(imported.file.len() as u64));
    Ok(back)
}

fn print_imported(imported: &openrage_app::obj::Imported, several: bool) {
    for t in &imported.targets {
        print_target(t, several);
    }
    if !imported.unplaced.is_empty() {
        outln!("left out (named after no drawable; --drawable picks one for them): {}", imported.unplaced.join(", "));
    }
}

fn print_target(t: &openrage_app::obj::Target, several: bool) {
    use rage_formats::ydr_write;
    if t.materials.is_empty() {
        return;
    }
    let place = match t.place {
        _ if !several => String::new(),
        ydr_write::Place::Main => "body: ".to_owned(),
        ydr_write::Place::Child(_) => format!("{} (own model): ", t.names.join(", ")),
        ydr_write::Place::Entry(_) => format!("{}: ", t.names.join(", ")),
    };
    print_materials(t, &place);
    if !t.textures_kept.is_empty() {
        outln!("{place}kept the shader's own texture (it takes no other): {}", t.textures_kept.join(", "));
    }
    if let Some(bones) = t.bones() {
        print_bones(bones);
    }
    if matches!(t.place, ydr_write::Place::Child(_)) {
        outln!("parts {} → the model of {}", t.parts.join(", "), t.names.join(", "));
    }
    if t.has_weights() {
        outln!("{place}bone weights from the nearest vertex of the original mesh");
    }
}

fn print_materials(t: &openrage_app::obj::Target, place: &str) {
    for (material, m) in &t.materials {
        let texture =
            m.texture.as_ref().map(|t| format!(", texture {} ({}×{})", t.name, t.width, t.height)).unwrap_or_default();
        outln!("{place}material {material:<24} → shader {}{texture}", m.shader);
    }
}

fn print_bones(bones: &openrage_app::obj::Bones) {
    for (part, bone) in &bones.matched {
        outln!("part {part:<28} → bone {bone}");
    }
    if !bones.unmatched.is_empty() {
        outln!("parts on the body ({}): {}", bones.body_name(), bones.unmatched.join(", "));
    }
    if !bones.left_out.is_empty() {
        outln!("left out (the game draws the wheel's own model there): {}", bones.left_out.join(", "));
    }
}

/// Reads the written model back and reports the closest level of each target.
fn print_closest_levels(
    back: &rage_resource::Resource,
    targets: &[openrage_app::obj::Target],
    several: bool,
) -> Result<()> {
    for t in targets.iter().filter(|t| !t.materials.is_empty()) {
        let check = rage_formats::ydr_write::read_at(back, t.place)?;
        let vertices: usize = check.meshes.iter().map(|m| m.positions.len()).sum();
        let triangles: usize = check.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let what =
            if several { format!("{}: ", t.names.first().map_or("body", String::as_str)) } else { String::new() };
        outln!("{what}closest level: {} meshes, {vertices} vertices, {triangles} triangles", check.meshes.len());
    }
    Ok(())
}

/// `file` (the model built from `obj`) to `--out` and/or the mods folder.
fn save_model(
    game_dir: &Path,
    vpath: &str,
    obj: &Path,
    file: Vec<u8>,
    o: &ReplaceOptions,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    if let Some(out) = o.out {
        fs::write(out, &file).with_context(|| format!("writing {}", out.display()))?;
        outln!("Saved {}", out.display());
    }
    if o.mods {
        let name = obj.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        cmd_edit(game_dir, vpath, Op::Set(file), &format!("Replaced the meshes of {vpath} with {name}"), keys)?;
    }
    Ok(())
}

/// `res` (a `.ybn`, or a `.ydr` with `ydr`) with its collision built from
/// the OBJ file `col`.
fn with_collision(
    game_dir: &Path,
    vpath: &str,
    res: &rage_resource::Resource,
    ydr: bool,
    col: &Path,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<Vec<u8>> {
    use rage_formats::ybn_write;
    let text = fs::read_to_string(col).with_context(|| format!("reading {}", col.display()))?;
    let meshes = openrage_app::obj::read(&text).with_context(|| format!("reading {}", col.display()))?;
    let names = openrage_app::models::collision_material_names(game_dir, keys.clone());
    let (mesh, matched) = openrage_app::obj::collision_mesh(&meshes, names);
    for (material, name) in &matched {
        match name {
            Some(n) => outln!("collision material {material:<20} → {n}"),
            None => outln!("collision material {material:<20} → the original's first (no such collision material)"),
        }
    }
    let donor = (!ybn_write::can_donate(res, ydr))
        .then(|| openrage_app::obj::find_collision_donor(game_dir, vpath, keys))
        .flatten();
    let file = ybn_write::replace_collision(res, ydr, &mesh, donor.as_ref().map(|(r, y)| (r, *y)))?;
    let back = rage_resource::Resource::from_rsc7(&file)?;
    let at = ybn_write::collision_of(&back, ydr)?;
    for b in ybn_write::bvh_bounds(&back, at) {
        ybn_write::check_bvh(&back, b).context("the written collision")?;
    }
    let (_, stats) = rage_formats::ybn::parse_at(&back, at).context("the written collision does not read back")?;
    outln!("collision: {} triangles, {} vertices", stats.triangles, stats.vertices);
    Ok(file)
}

pub(crate) fn cmd_put(game_dir: &Path, file: &Path, target: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let data = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let name = file.file_name().context("file has no name")?.to_string_lossy();
    let target = if target.ends_with('/') { format!("{target}{name}") } else { target.to_owned() };
    cmd_edit(game_dir, &target, Op::Set(data), &format!("Imported {name} into {target}"), keys)
}

pub(crate) fn cmd_mv(game_dir: &Path, from: &str, to: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let a = install::split(game_dir, from).context("source is not inside an archive")?;
    // `to` may be a full virtual path or a path inside the same archive.
    let to_inner = match install::split(game_dir, to) {
        Some(b) if b.archive.eq_ignore_ascii_case(&a.archive) => b.inner,
        Some(_) => bail!("cannot move between archives"),
        None => to.trim_start_matches('/').to_owned(),
    };
    let started = Instant::now();
    let applied = install::apply(
        game_dir,
        keys,
        &a.archive,
        &[(a.inner.clone(), Op::Rename(to_inner.clone()))],
        &format!("Moved {} to {to_inner}", a.inner),
    )?;
    report(&applied, started);
    Ok(())
}

pub(crate) fn cmd_history(game_dir: &Path) -> Result<()> {
    use openrage_app::journal::Kind;
    let entries = match rage_mods::mods_dir(game_dir) {
        Some(mods) => openrage_app::journal::Journal::new(&mods).entries()?,
        None if json() => Vec::new(),
        None => {
            outln!("No mods folder.");
            return Ok(());
        }
    };
    if entries.is_empty() && !json() {
        outln!("No recorded changes.");
    }
    let mut out = Rows::start();
    for e in entries {
        let in_game = matches!(e.kind, Kind::Edited { in_game: true, .. } | Kind::FileWritten { in_game: true, .. });
        let place = if in_game { " (game folder)" } else { "" };
        out.row(
            || format!("#{:<4} {}{place}  {}", e.id, e.archive, e.description),
            serde_json::json!({
                "id": e.id,
                "time": e.time,
                "path": e.archive,
                "in_game_folder": in_game,
                "description": e.description,
                "group": e.group,
            }),
        );
    }
    out.finish();
    Ok(())
}

pub(crate) fn cmd_write_target(game_dir: &Path, target: Option<&str>) -> Result<()> {
    let folder = rage_mods::overlay::folder_name(game_dir);
    let target = match target {
        None => install::write_target(game_dir),
        Some(t) if t.eq_ignore_ascii_case("game") => {
            install::set_write_target(game_dir, install::WriteTarget::Game)?;
            install::WriteTarget::Game
        }
        Some(t) if t.eq_ignore_ascii_case(folder) || t.eq_ignore_ascii_case("update") => {
            install::set_write_target(game_dir, install::WriteTarget::Overlay)?;
            install::WriteTarget::Overlay
        }
        Some(t) => bail!("unknown write target {t:?}: use `{folder}` or `game`"),
    };
    match target {
        install::WriteTarget::Overlay => {
            outln!("Changes go into the {folder} folder; the game folder stays untouched.")
        }
        install::WriteTarget::Game => outln!(
            "Changes go into the game's own files. The original of every changed file is kept first \
             (`openrage originals` lists them, `--restore` puts one back)."
        ),
    }
    Ok(())
}

/// Asks before a change; `yes` (from `--yes`) skips the question. Without a
/// terminal to ask on, the command fails instead of guessing, and declining
/// is an error too, so scripts see that nothing was done.
pub(crate) fn confirm(question: &str, yes: bool) -> Result<()> {
    use std::io::IsTerminal;
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("{question} needs confirmation: pass --yes to run without asking");
    }
    eprint!("{question} [y/N] ");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        bail!("cancelled; nothing was changed");
    }
    Ok(())
}

pub(crate) fn cmd_originals(game_dir: &Path, restore: Option<&str>, restore_all: bool, yes: bool) -> Result<()> {
    let kept = install::originals(game_dir)?;
    if let Some(rel) = restore {
        confirm(&format!("Put the original of {rel} back? Its changes leave the history."), yes)?;
        let path = install::restore_original(game_dir, rel)?;
        outln!("Restored {} (its changes are no longer in the history).", path.display());
        return Ok(());
    }
    if restore_all {
        if kept.is_empty() {
            outln!("No game files were changed in place.");
            return Ok(());
        }
        confirm(&format!("Put {} original(s) back? Their changes leave the history.", kept.len()), yes)?;
        for rel in &kept {
            install::restore_original(game_dir, rel)?;
            outln!("Restored {rel}");
        }
        return Ok(());
    }
    if kept.is_empty() {
        outln!("No game files were changed in place.");
    }
    for rel in kept {
        let changed = install::differs_from_original(game_dir, &rel);
        outln!("{rel}  {}", if changed { "changed" } else { "same as the original" });
    }
    Ok(())
}

pub(crate) fn cmd_undo(game_dir: &Path) -> Result<()> {
    match install::undo_last(game_dir)? {
        Some(e) => outln!("Undone: {} ({})", e.description, e.archive),
        None => outln!("Nothing to undo."),
    }
    Ok(())
}

/// Writes an edited placement text back into the `.wpl` at `path`.
pub(crate) fn cmd_import_wpl(game_dir: &Path, path: &str, file: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    if !path.to_ascii_lowercase().ends_with(".wpl") {
        bail!("--import works for GTA IV placement files (.wpl)");
    }
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let original = match rage_mods::resolve(game_dir, path, keys.clone())?.0 {
        Node::Gta4File(a, id) => a.read(id)?,
        Node::PlainFile(p) => fs::read(p)?,
        _ => bail!("{path} is not a file"),
    };
    let data = openrage_app::gta4::wpl_from_text(&text, Some(&original))?;
    if data == original {
        outln!("No changes.");
        return Ok(());
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    cmd_edit(game_dir, path, Op::Set(data), &format!("Edited {name}"), keys)
}
