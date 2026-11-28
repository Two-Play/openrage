//! `openrage export-openformats`, `import-openformats` and
//! `verify --models --openformats`: OpenIV's text formats.

use std::collections::HashMap;

use openrage_app::openformats::{self as ofs, ImportOptions, Kind};
use rage_formats::drawable::{self, Drawable};
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
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let total = merge_stats(run_workers(&starts));
    report_verify(&total, started)
}

#[derive(Default)]
struct VerifyStats {
    text: usize,
    imported: usize,
    bytes: usize,
    /// Why a model could not be written back (the model writer's limits).
    not_writable: HashMap<String, (usize, String)>,
    errors: Vec<String>,
}

/// A game model with its openFormats export, as the checks see it.
struct ModelFile<'a> {
    res: &'a Resource,
    name: &'a str,
    flags: (u32, u32),
    src: of::Source<'a>,
    main: &'a str,
}

/// Exports one game file as openFormats text and checks it reads back the same,
/// then (Enhanced drawables and fragments) imports it back onto itself.
fn verify_file(res: &Resource, name: &str, flags: (u32, u32), s: &mut VerifyStats) -> Result<()> {
    let kind = Kind::of_game_file(name).expect("filtered");
    let files = ofs::export(res, name)?;
    s.bytes += files.iter().map(|f| f.data.len()).sum::<usize>();
    let map: HashMap<&str, &[u8]> = files.iter().map(|f| (f.path.as_str(), f.data.as_slice())).collect();
    let src = |p: &str| -> Result<Vec<u8>, ResourceError> {
        map.get(p).map(|d| d.to_vec()).ok_or_else(|| ResourceError::Format(format!("{p} was not written")))
    };
    let main = files.last().expect("main file").path.clone();
    let model = ModelFile { res, name, flags, src: &src, main: &main };
    text_round_trip(kind, &model)?;
    s.text += 1;
    // Back onto the game file.
    let enhanced = res.version == drawable::VERSION_YDR_ENHANCED || res.version == drawable::VERSION_YFT_ENHANCED;
    if !enhanced || !matches!(kind, Kind::Drawable | Kind::Fragment) {
        return Ok(());
    }
    import_back(&model, s)
}

fn text_round_trip(kind: Kind, m: &ModelFile) -> Result<()> {
    let stem = m.name.rsplit_once('.').map_or(m.name, |(s, _)| s);
    match kind {
        Kind::Drawable => {
            if let Some(d) = of::difference(&of::read_ydr(m.res, stem)?, &of::parse_odr(m.src, m.main)?) {
                bail!("{d}");
            }
            Ok(())
        }
        Kind::Fragment => fragment_round_trip(m, stem),
        Kind::Dictionary => dictionary_round_trip(m),
        Kind::Textures => textures_round_trip(m),
    }
}

fn fragment_round_trip(m: &ModelFile, stem: &str) -> Result<()> {
    let (a, b) = (of::read_yft(m.res, stem)?, of::parse_oft(m.src, m.main)?);
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
    Ok(())
}

fn dictionary_round_trip(m: &ModelFile) -> Result<()> {
    let (a, b) = (of::read_ydd(m.res)?, of::parse_odd(m.src, m.main)?);
    if a.len() != b.len() {
        bail!("{} drawables, {} read back", a.len(), b.len());
    }
    for (x, y) in a.iter().zip(&b) {
        if let Some(d) = of::difference(x, y) {
            bail!("{}: {d}", x.name);
        }
    }
    Ok(())
}

fn textures_round_trip(m: &ModelFile) -> Result<()> {
    let (a, b) = (of::read_ytd(m.res)?, of::parse_otd(m.src, m.main)?);
    if a.len() != b.len() {
        bail!("{} textures, {} read back", a.len(), b.len());
    }
    for (x, y) in a.iter().zip(&b) {
        if let Some(d) = of::texture_difference(&x.texture, &y.texture) {
            bail!("{d}");
        }
    }
    Ok(())
}

/// Imports the model back onto itself and compares what the game reads from it.
fn import_back(m: &ModelFile, s: &mut VerifyStats) -> Result<()> {
    let res = m.res;
    let options = ImportOptions { always_rebuild_meshes: true };
    let imported = match ofs::import_with(res, m.flags, m.name, m.src, m.main, options) {
        Ok(i) => i,
        Err(e) => {
            note_not_writable(s, m.name, &e.to_string());
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
}

/// The model writer's limits: grouped by reason.
fn note_not_writable(s: &mut VerifyStats, name: &str, why: &str) {
    let key: String = why.split(|c: char| c.is_ascii_digit()).collect::<Vec<_>>().join("#");
    let entry = s.not_writable.entry(key).or_insert((0, String::new()));
    entry.0 += 1;
    if entry.1.is_empty() {
        entry.1 = format!("{name}: {why}");
    }
}

// Archives are read in parallel, but one archive holds thousands of
// models (all vehicles are in one): the files go through a short queue
// to one worker per core, so a big archive does not leave the others idle.
type Job = (String, String, (u32, u32), Result<Resource, String>);

fn run_workers(starts: &[Start]) -> Vec<VerifyStats> {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let (send, receive) = std::sync::mpsc::sync_channel::<Job>(workers * 2);
    let receive = std::sync::Mutex::new(receive);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(|| verify_worker(&receive))).collect();
        let read_errors = queue_models(starts, &send);
        drop(send);
        let mut all: Vec<VerifyStats> = handles.into_iter().map(|h| h.join().expect("verify worker")).collect();
        all.push(VerifyStats { errors: read_errors, ..Default::default() });
        all
    })
}

fn verify_worker(receive: &std::sync::Mutex<std::sync::mpsc::Receiver<Job>>) -> VerifyStats {
    let mut s = VerifyStats::default();
    loop {
        let job = receive.lock().map_err(|_| ()).and_then(|r| r.recv().map_err(|_| ()));
        let Ok((path, name, flags, res)) = job else { break };
        let result = res.map_err(|e| anyhow::anyhow!(e)).and_then(|res| verify_file(&res, &name, flags, &mut s));
        if let Err(e) = result {
            s.errors.push(format!("{path}: {e:#}"));
        }
    }
    s
}

/// Sends every model and texture dictionary below `starts` to the workers;
/// returns what could not be listed.
fn queue_models(starts: &[Start], send: &std::sync::mpsc::SyncSender<Job>) -> Vec<String> {
    starts
        .par_iter()
        .flat_map_iter(|(prefix, rpf, dir)| {
            let mut nested = Vec::new();
            let send = send.clone();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let entry = a.entry(id);
                    let rage_rpf::EntryKind::Resource { system_flags, graphics_flags, .. } = entry.kind else {
                        return;
                    };
                    if Kind::of_game_file(&entry.name).is_none() {
                        return;
                    }
                    let res = a.read_resource(id).map_err(|e| e.to_string());
                    let _ = send.send((p.to_owned(), entry.name.clone(), (system_flags, graphics_flags), res));
                },
                &mut |e| nested.push(e.to_string()),
            );
            nested
        })
        .collect()
}

fn merge_stats(all: Vec<VerifyStats>) -> VerifyStats {
    let mut total = VerifyStats::default();
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
    total
}

fn report_verify(total: &VerifyStats, started: Instant) -> Result<()> {
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    for (kind, n) in failure_kinds(&total.errors).iter().take(30) {
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

/// Kinds of failures (the message without the file and the numbers), most frequent first.
fn failure_kinds(errors: &[String]) -> Vec<(String, usize)> {
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for e in errors {
        let message = e.split_once(": ").map_or(e.as_str(), |(_, m)| m);
        let message = message.rsplit_once(" differs ").map_or(message.to_owned(), |(_, m)| format!("differs {m}"));
        let kind: String = message.split(|c: char| c.is_ascii_digit()).collect::<Vec<_>>().join("#");
        *kinds.entry(kind).or_default() += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    kinds
}

/// What the game reads from a model imported back onto itself must be the
/// original: the same meshes (to the text's precision), shaders, textures
/// and bones.
fn compare_read_back(a: &Drawable, b: &Drawable) -> Result<()> {
    if a.meshes.len() != b.meshes.len() {
        bail!("{} meshes, {} after importing", a.meshes.len(), b.meshes.len());
    }
    for (i, (x, y)) in a.meshes.iter().zip(&b.meshes).enumerate() {
        compare_mesh(i, x, y)?;
    }
    let names = |d: &Drawable| -> Vec<(u32, Vec<String>)> {
        d.shaders.iter().map(|s| (s.name_hash, s.textures.clone())).collect()
    };
    if names(a) != names(b) {
        bail!("shaders differ after importing");
    }
    let bones = |d: &Drawable| -> Vec<(String, u16, i16)> {
        d.skeleton.iter().flat_map(|s| &s.bones).map(|b| (b.name.clone(), b.tag, b.parent)).collect()
    };
    if bones(a) != bones(b) {
        bail!("bones differ after importing");
    }
    let textures = |d: &Drawable| -> Vec<(String, usize)> {
        d.textures.iter().flat_map(|t| &t.textures).map(|t| (t.name.clone(), t.data.len())).collect()
    };
    if textures(a) != textures(b) {
        bail!("embedded textures differ after importing");
    }
    Ok(())
}

fn close(x: f32, y: f32) -> bool {
    x == y || (x - y).abs() <= 1e-4 + 1e-5 * x.abs().max(y.abs()) || (x.is_nan() && y.is_nan())
}

fn close_all(x: &[f32], y: &[f32]) -> bool {
    x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(*p, *q))
}

fn near_normals(p: &[[f32; 3]], q: &[[f32; 3]]) -> bool {
    p.len() == q.len() && p.iter().zip(q).all(|(u, v)| (0..3).all(|k| (u[k] - v[k]).abs() < 0.01 || u[k].is_nan()))
}

fn compare_mesh(i: usize, x: &drawable::Mesh, y: &drawable::Mesh) -> Result<()> {
    if x.indices != y.indices || x.shader != y.shader {
        bail!("mesh {i}: triangles or shader differ after importing");
    }
    if !close_all(x.positions.as_flattened(), y.positions.as_flattened()) {
        bail!("mesh {i}: positions differ after importing");
    }
    compare_mesh_attributes(i, x, y)
}

fn compare_mesh_attributes(i: usize, x: &drawable::Mesh, y: &drawable::Mesh) -> Result<()> {
    if !x.uvs.is_empty() && !close_all(x.uvs.as_flattened(), y.uvs.as_flattened()) {
        bail!("mesh {i}: texture coordinates differ after importing");
    }
    if !x.normals.is_empty() && !near_normals(&x.normals, &y.normals) {
        bail!("mesh {i}: normals differ after importing");
    }
    if !x.colors.is_empty() && !close_all(x.colors.as_flattened(), y.colors.as_flattened()) {
        bail!("mesh {i}: colours differ after importing");
    }
    Ok(())
}
