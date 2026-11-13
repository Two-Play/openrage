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
        Node::ArchiveFile(rpf, id) if openrage_app::textures::gta5_model_ext(path).is_some() => {
            openrage_app::textures::gta5_model_textures(&rpf.read_resource(id)?, path)
                .with_context(|| format!("{path} has no textures of its own"))
        }
        Node::ArchiveFile(rpf, id) => Ok(rage_formats::ytd::parse(&rpf.read_resource(id)?)?),
        Node::Rpf8File(rpf, i) => {
            let entry = &rpf.entries().context("no entries")?[i];
            Ok(rage_formats::ytd::parse_rdr2(&rpf.read_resource(entry)?)?)
        }
        Node::Gta4File(a, id) if openrage_app::textures::is_gta4_model(path) => {
            openrage_app::textures::gta4_model_textures(&a.read_resource(id)?)
                .with_context(|| format!("{path} has no textures of its own"))
        }
        Node::Gta4File(a, id) => Ok(rage_formats::ytd::parse_gta4(&a.read_resource(id)?)?),
        _ => bail!("{path} is not a file inside an archive"),
    }
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
        outln!("Copied the archive into the overlay folder first (mods; GTA IV: update).");
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
}

pub(crate) fn cmd_replace_model(
    game_dir: &Path,
    vpath: &str,
    obj: &Path,
    o: ReplaceOptions,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use openrage_app::obj;
    use rage_formats::{drawable, ydr_write};
    let Node::ArchiveFile(rpf, id) = rage_mods::resolve(game_dir, vpath, keys.clone())?.0 else {
        bail!("{vpath} is not a file inside an archive")
    };
    if !vpath.to_ascii_lowercase().ends_with(".ydr") {
        bail!("{vpath} is not a drawable (.ydr)");
    }
    if o.lods.len() > 3 {
        bail!("at most three further detail levels (--lod)");
    }
    let template = rpf.read_resource(id)?;
    let shaders = ydr_write::usable_shaders(&template)?;
    // Every level's OBJ, and the textures their materials name.
    let mut files = vec![obj.to_path_buf()];
    files.extend(o.lods.iter().cloned());
    let mut levels_text = Vec::new();
    let mut textures = std::collections::HashMap::new();
    for f in &files {
        let text = fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        if o.textures {
            let dir = f.parent().unwrap_or(Path::new("."));
            for (material, mtl) in obj::materials(&text, dir) {
                if textures.contains_key(&material) {
                    continue;
                }
                let texture = match (mtl.texture, mtl.colour) {
                    (Some(png), _) => {
                        let name = obj::texture_name(&png.file_stem().unwrap_or_default().to_string_lossy());
                        let (w, h, rgba) =
                            obj::read_png(&png).with_context(|| format!("texture of material {material}"))?;
                        ydr_write::texture_from_rgba(&name, w, h, &rgba)?
                    }
                    // Only a colour: a small texture of that colour.
                    (None, Some(c)) => {
                        let px = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                        let rgba: Vec<u8> = (0..16).flat_map(|_| [px[0], px[1], px[2], 255]).collect();
                        ydr_write::texture_from_rgba(&format!("{}_colour", obj::texture_name(&material)), 4, 4, &rgba)?
                    }
                    (None, None) => continue,
                };
                textures.insert(material, texture);
            }
        }
        levels_text.push(text);
    }
    let mut fitter = obj::Fitter::new(&shaders, o.shader, &textures)?;
    let mut levels = Vec::new();
    for text in &levels_text {
        levels.push(fitter.level(obj::read(text)?));
    }
    for (material, m) in &fitter.materials {
        let texture =
            m.texture.as_ref().map(|t| format!(", texture {} ({}×{})", t.name, t.width, t.height)).unwrap_or_default();
        outln!("material {material:<24} → shader {}{texture}", m.shader);
    }
    let materials: Vec<ydr_write::NewMaterial> = fitter.materials.into_iter().map(|(_, m)| m).collect();
    let donor = (materials.iter().any(|m| m.texture.is_some()) && !ydr_write::can_donate_textures(&template))
        .then(|| obj::find_texture_donor(game_dir, vpath, keys.clone()))
        .flatten();
    let level_refs: Vec<&[ydr_write::NewMesh]> = levels.iter().map(Vec::as_slice).collect();
    let file = ydr_write::replace_lods(&template, &level_refs, Some(&materials), donor.as_ref())?;
    // What the game will read: check it reads back.
    let check = drawable::parse_ydr(&rage_resource::Resource::from_rsc7(&file)?)?;
    let vertices: usize = check.meshes.iter().map(|m| m.positions.len()).sum();
    let triangles: usize = check.meshes.iter().map(|m| m.indices.len() / 3).sum();
    outln!(
        "{} detail level(s); closest: {} meshes, {vertices} vertices, {triangles} triangles ({})",
        levels.len(),
        check.meshes.len(),
        human(file.len() as u64)
    );
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
