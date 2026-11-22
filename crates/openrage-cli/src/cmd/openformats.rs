//! `openrage export-openformats`, `import-openformats` and
//! `verify --models --openformats`: OpenIV's text formats.

use std::collections::HashMap;

use openrage_app::openformats::{self as ofs, ImportOptions, Kind};
use rage_formats::openformats as of;
use rage_resource::{Resource, ResourceError};

use crate::*;

/// A game file inside an archive: its resource and page flags.
fn read_game_file(game_dir: &Path, vpath: &str, keys: Option<Arc<GtaVKeys>>) -> Result<(Resource, (u32, u32))> {
    let Node::ArchiveFile(rpf, id) = rage_mods::resolve(game_dir, vpath, keys)?.0 else {
        bail!("{vpath} is not a file inside an archive")
    };
    let rage_rpf::EntryKind::Resource { system_flags, graphics_flags, .. } = rpf.entry(id).kind else {
        bail!("{vpath} is not a resource")
    };
    Ok((rpf.read_resource(id)?, (system_flags, graphics_flags)))
}

fn file_name(vpath: &str) -> &str {
    vpath.rsplit('/').next().unwrap_or(vpath)
}

pub(crate) fn cmd_export_openformats(
    game_dir: &Path,
    vpath: &str,
    out: &Path,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let (res, _) = read_game_file(game_dir, vpath, keys)?;
    let files = ofs::export(&res, file_name(vpath))?;
    let main = ofs::write_files(out, &files)?;
    let bytes: usize = files.iter().map(|f| f.data.len()).sum();
    outln!("Exported {} files ({}) to {}", files.len(), human(bytes as u64), out.display());
    outln!("Main file: {}", main.display());
    Ok(())
}

pub(crate) fn cmd_import_openformats(
    game_dir: &Path,
    vpath: &str,
    file: &Path,
    out: Option<&Path>,
    mods: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let (res, flags) = read_game_file(game_dir, vpath, keys.clone())?;
    let imported = ofs::import(&res, flags, file_name(vpath), file)?;
    for n in &imported.notes {
        outln!("{n}");
    }
    let Some(data) = imported.file else {
        outln!("Nothing to write: the files are the game file's.");
        return Ok(());
    };
    if let Some(out) = out {
        fs::write(out, &data).with_context(|| format!("writing {}", out.display()))?;
        outln!("Saved {} ({})", out.display(), human(data.len() as u64));
    }
    if mods {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        cmd_edit(game_dir, vpath, Op::Set(data), &format!("Imported {name} into {vpath}"), keys)?;
    }
    Ok(())
}

/// Every model and texture dictionary of the game as openFormats text and
/// back: the text must read back as the same model. GTA V Enhanced drawables
/// and fragments are also imported back onto themselves (meshes written
/// anew), and what the game reads from the result must match the original.
pub(crate) fn cmd_verify_openformats(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::drawable;
    #[derive(Default)]
    struct Stats {
        text: usize,
        imported: usize,
        bytes: usize,
        /// Why a model could not be written back (the model writer's limits).
        not_writable: HashMap<String, (usize, String)>,
        errors: Vec<String>,
    }
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let check = |res: &Resource, name: &str, flags: (u32, u32), s: &mut Stats| -> Result<()> {
        let kind = Kind::of_game_file(name).expect("filtered");
        let files = ofs::export(res, name)?;
        s.bytes += files.iter().map(|f| f.data.len()).sum::<usize>();
        let map: HashMap<&str, &[u8]> = files.iter().map(|f| (f.path.as_str(), f.data.as_slice())).collect();
        let src = |p: &str| -> Result<Vec<u8>, ResourceError> {
            map.get(p).map(|d| d.to_vec()).ok_or_else(|| ResourceError::Format(format!("{p} was not written")))
        };
        let main = files.last().expect("main file").path.clone();
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        // Text round trip.
        match kind {
            Kind::Drawable => {
                if let Some(d) = of::difference(&of::read_ydr(res, stem)?, &of::parse_odr(&src, &main)?) {
                    bail!("{d}");
                }
            }
            Kind::Fragment => {
                let (a, b) = (of::read_yft(res, stem)?, of::parse_oft(&src, &main)?);
                if a.children.len() != b.children.len() {
                    bail!("{} parts, {} read back", a.children.len(), b.children.len());
                }
                for ((ta, da), (tb, db)) in std::iter::once((&0, &a.drawable))
                    .chain(a.children.iter().map(|(t, d)| (t, d)))
                    .zip(std::iter::once((&0, &b.drawable)).chain(b.children.iter().map(|(t, d)| (t, d))))
                {
                    if ta != tb {
                        bail!("part on bone {ta} read back on bone {tb}");
                    }
                    if let Some(d) = of::difference(da, db) {
                        bail!("{}: {d}", da.name);
                    }
                }
            }
            Kind::Dictionary => {
                let (a, b) = (of::read_ydd(res)?, of::parse_odd(&src, &main)?);
                if a.len() != b.len() {
                    bail!("{} drawables, {} read back", a.len(), b.len());
                }
                for (x, y) in a.iter().zip(&b) {
                    if let Some(d) = of::difference(x, y) {
                        bail!("{}: {d}", x.name);
                    }
                }
            }
            Kind::Textures => {
                let (a, b) = (of::read_ytd(res)?, of::parse_otd(&src, &main)?);
                if a.len() != b.len() {
                    bail!("{} textures, {} read back", a.len(), b.len());
                }
                for (x, y) in a.iter().zip(&b) {
                    if let Some(d) = of::texture_difference(&x.texture, &y.texture) {
                        bail!("{d}");
                    }
                }
            }
        }
        s.text += 1;
        // Back onto the game file.
        let enhanced = res.version == drawable::VERSION_YDR_ENHANCED || res.version == drawable::VERSION_YFT_ENHANCED;
        if !enhanced || !matches!(kind, Kind::Drawable | Kind::Fragment) {
            return Ok(());
        }
        let options = ImportOptions { always_rebuild_meshes: true };
        let imported = match ofs::import_with(res, flags, name, &src, &main, options) {
            Ok(i) => i,
            Err(e) => {
                // The model writer's limits: grouped by reason.
                let why = e.to_string();
                let key: String = why.split(|c: char| c.is_ascii_digit()).collect::<Vec<_>>().join("#");
                let entry = s.not_writable.entry(key).or_insert((0, String::new()));
                entry.0 += 1;
                if entry.1.is_empty() {
                    entry.1 = format!("{name}: {why}");
                }
                return Ok(());
            }
        };
        let file = imported.file.context("nothing written")?;
        let (before, after) = if res.version == drawable::VERSION_YFT_ENHANCED {
            (drawable::parse_yft(res)?, rage_formats::ydr_write::read_back(&file)?)
        } else {
            (drawable::parse_ydr(res)?, rage_formats::ydr_write::read_back(&file)?)
        };
        compare_read_back(&before, &after)?;
        s.imported += 1;
        Ok(())
    };
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut s = Stats::default();
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let entry = a.entry(id);
                    let rage_rpf::EntryKind::Resource { system_flags, graphics_flags, .. } = entry.kind else { return };
                    if Kind::of_game_file(&entry.name).is_none() {
                        return;
                    }
                    let name = entry.name.clone();
                    let result = a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|res| check(&res, &name, (system_flags, graphics_flags), &mut s));
                    if let Err(e) = result {
                        s.errors.push(format!("{p}: {e:#}"));
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            s.errors.extend(nested);
            s
        })
        .collect();
    let mut total = Stats::default();
    for s in all {
        total.text += s.text;
        total.imported += s.imported;
        total.bytes += s.bytes;
        total.errors.extend(s.errors);
        for (k, (n, example)) in s.not_writable {
            let e = total.not_writable.entry(k).or_insert((0, example));
            e.0 += n;
        }
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    // Kinds of failures (the message without the file and the numbers).
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for e in &total.errors {
        let message = e.split_once(": ").map_or(e.as_str(), |(_, m)| m);
        let message = message.rsplit_once(" differs ").map_or(message.to_owned(), |(_, m)| format!("differs {m}"));
        let kind: String = message.split(|c: char| c.is_ascii_digit()).collect::<Vec<_>>().join("#");
        *kinds.entry(kind).or_default() += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (kind, n) in kinds.iter().take(30) {
        outln!("{n:>7}× {kind}");
    }
    let mut reasons: Vec<_> = total.not_writable.values().collect();
    reasons.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    let skipped: usize = reasons.iter().map(|(n, _)| n).sum();
    for (n, example) in reasons.iter().take(20) {
        outln!("not written back ({n}×), e.g. {example}");
    }
    outln!(
        "{} files as openFormats text and back ({}), {} Enhanced models imported back onto themselves, {skipped} beyond the model writer, {} errors ({:.1}s)",
        total.text,
        human(total.bytes as u64),
        total.imported,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}

/// What the game reads from a model imported back onto itself must be the
/// original: the same meshes (to the text's precision), shaders, textures
/// and bones.
fn compare_read_back(a: &rage_formats::drawable::Drawable, b: &rage_formats::drawable::Drawable) -> Result<()> {
    let close =
        |x: f32, y: f32| x == y || (x - y).abs() <= 1e-4 + 1e-5 * x.abs().max(y.abs()) || (x.is_nan() && y.is_nan());
    let close_all = |x: &[f32], y: &[f32]| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(*p, *q));
    if a.meshes.len() != b.meshes.len() {
        bail!("{} meshes, {} after importing", a.meshes.len(), b.meshes.len());
    }
    for (i, (x, y)) in a.meshes.iter().zip(&b.meshes).enumerate() {
        if x.indices != y.indices || x.shader != y.shader {
            bail!("mesh {i}: triangles or shader differ after importing");
        }
        if !close_all(x.positions.as_flattened(), y.positions.as_flattened()) {
            bail!("mesh {i}: positions differ after importing");
        }
        if !x.uvs.is_empty() && !close_all(x.uvs.as_flattened(), y.uvs.as_flattened()) {
            bail!("mesh {i}: texture coordinates differ after importing");
        }
        let near = |p: &[[f32; 3]], q: &[[f32; 3]]| {
            p.len() == q.len()
                && p.iter().zip(q).all(|(u, v)| (0..3).all(|k| (u[k] - v[k]).abs() < 0.01 || u[k].is_nan()))
        };
        if !x.normals.is_empty() && !near(&x.normals, &y.normals) {
            bail!("mesh {i}: normals differ after importing");
        }
        if !x.colors.is_empty() && !close_all(x.colors.as_flattened(), y.colors.as_flattened()) {
            bail!("mesh {i}: colours differ after importing");
        }
    }
    let names = |d: &rage_formats::drawable::Drawable| -> Vec<(u32, Vec<String>)> {
        d.shaders.iter().map(|s| (s.name_hash, s.textures.clone())).collect()
    };
    if names(a) != names(b) {
        bail!("shaders differ after importing");
    }
    let bones = |d: &rage_formats::drawable::Drawable| -> Vec<(String, u16, i16)> {
        d.skeleton.iter().flat_map(|s| &s.bones).map(|b| (b.name.clone(), b.tag, b.parent)).collect()
    };
    if bones(a) != bones(b) {
        bail!("bones differ after importing");
    }
    let textures = |d: &rage_formats::drawable::Drawable| -> Vec<(String, usize)> {
        d.textures.iter().flat_map(|t| &t.textures).map(|t| (t.name.clone(), t.data.len())).collect()
    };
    if textures(a) != textures(b) {
        bail!("embedded textures differ after importing");
    }
    Ok(())
}
