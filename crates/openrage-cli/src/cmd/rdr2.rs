//! Red Dead Redemption 2: archives, keys, names and peds.

use crate::*;

/// Which files `openrage rpf8` parses instead of only reading them.
#[derive(Clone, Copy)]
pub(crate) struct Checks {
    pub(crate) textures: bool,
    pub(crate) models: bool,
    pub(crate) texts: bool,
    pub(crate) meta: bool,
    pub(crate) audio: bool,
    pub(crate) anims: bool,
}

pub(crate) fn cmd_rpf8(path: &Path, list: bool, checks: Checks) -> Result<()> {
    let Checks { textures, models, texts: text, meta, audio, anims } = checks;
    use rage_rpf::rpf8::{Compression, Rpf8};
    let keys = openrage_app::rdr2::load_keys()?;
    let start = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    let names = openrage_app::rdr2::game_folder(start).and_then(|g| openrage_app::rdr2::load_names(&g));
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                collect(&p, out)?;
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("rpf")) {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if path.is_dir() {
        collect(path, &mut files)?;
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    /// Counts and report lines for one archive file and everything nested in it.
    #[derive(Default)]
    struct Tally {
        archives: usize,
        nested: usize,
        readable: usize,
        entries: usize,
        failed: usize,
        /// Textures decoded, models parsed or text tables read.
        textures: usize,
        /// Triangles of the models, or strings of the text tables.
        triangles: usize,
        lines: Vec<String>,
    }
    /// Parses a texture dictionary and decodes all its textures; returns how many.
    fn check_textures(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
        let dict = rage_formats::ytd::parse_rdr2(&res).map_err(|err| err.to_string())?;
        if let Some(err) = dict.errors.first() {
            return Err(format!(
                "{} of {} textures unreadable, e.g. {err}",
                dict.errors.len(),
                dict.errors.len() + dict.textures.len()
            ));
        }
        for t in &dict.textures {
            if t.to_rgba().is_none() {
                return Err(format!("{}: cannot decode {}", t.name, t.format.name()));
            }
        }
        Ok(dict.textures.len())
    }
    /// Parses a model; returns its triangle count.
    fn check_model(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        use rage_formats::drawable;
        let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
        let drawables = match e.ext_id {
            3 => vec![drawable::parse_fragment_rdr2(&res).map_err(|err| err.to_string())?.drawable],
            4 => drawable::parse_ydd_rdr2(&res).map_err(|err| err.to_string())?.into_iter().map(|(_, d)| d).collect(),
            _ => vec![drawable::parse_ydr_rdr2(&res).map_err(|err| err.to_string())?],
        };
        let triangles: usize = drawables.iter().map(|d| d.triangle_count()).sum();
        if drawables.iter().all(|d| d.meshes.is_empty()) {
            return Err("no meshes".into());
        }
        for d in &drawables {
            for m in &d.meshes {
                if m.positions.iter().any(|p| p.iter().any(|v| !v.is_finite() || v.abs() > 1.0e6)) {
                    return Err("implausible vertex positions".into());
                }
            }
        }
        Ok(triangles)
    }
    /// Converts a metadata file to XML; returns how many problems it had.
    fn check_meta(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        let xml = if e.is_resource {
            let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
            if !rage_formats::meta::is_meta(res.version) {
                return Err(format!("resource version {} is not a meta file", res.version));
            }
            openrage_app::metadata::from_resource(&res).map_err(|err| err.to_string())?
        } else {
            openrage_app::metadata::from_bytes_rdr2(&rpf.read(e).map_err(|err| err.to_string())?)
                .map_err(|err| err.to_string())?
        };
        Ok(xml.errors.len())
    }
    /// Reads a text table; returns its string count.
    fn check_texts(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
        Ok(rage_formats::ldb::parse(&res).map_err(|err| err.to_string())?.len())
    }
    /// Decodes every sound of an audio container; `None` when it is not one.
    fn check_audio(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<Option<(usize, f32)>, String> {
        let data = rpf.read(e).map_err(|err| err.to_string())?;
        if !data.starts_with(b"ADAT") {
            return Ok(None);
        }
        let awc = rage_formats::awc::Awc::parse_rdr2(data).map_err(|err| err.to_string())?;
        if awc.stream_format.is_some() {
            let pcm = awc.decode_stream().map_err(|err| err.to_string())?;
            return Ok(Some((1, pcm.duration_secs())));
        }
        let mut secs = 0.0;
        for i in 0..awc.sounds.len() {
            secs +=
                awc.decode_sound(i).map_err(|err| format!("sound {:08x}: {err}", awc.sounds[i].id))?.duration_secs();
        }
        Ok(Some((awc.sounds.len(), secs)))
    }
    fn walk(rpf: &Rpf8, depth: usize, list: bool, checks: Checks, t: &mut Tally) {
        let Checks { textures, models, texts, meta, audio, anims } = checks;
        let h = rpf.header();
        let Some(entries) = rpf.entries() else { return };
        t.readable += 1;
        let indent = "  ".repeat(depth + 1);
        for e in entries {
            t.entries += 1;
            let name = e.display_name(h.platform);
            if list {
                let comp = match e.compression {
                    Compression::None => "",
                    Compression::Deflate => " deflate",
                    Compression::Oodle => " oodle",
                    Compression::Unknown(_) => " ?",
                };
                let enc = if e.is_encrypted() { " encrypted" } else { "" };
                t.lines.push(format!("{indent}{name:<60} {:>10}{comp}{enc}", e.size()));
            }
            if e.ext_id == 0 {
                match rpf.open_child(e) {
                    Ok(child) => {
                        t.nested += 1;
                        walk(&child, depth + 1, list, checks, t);
                    }
                    Err(err) => {
                        t.failed += 1;
                        t.lines.push(format!("{indent}{name}: {err}"));
                    }
                }
            } else if anims {
                // Extension id 67 is `.#cd` (`.ycd` on PC).
                if e.ext_id == 67 && e.is_resource {
                    match rpf
                        .read_resource(e)
                        .map_err(|err| err.to_string())
                        .and_then(|res| rage_formats::clip::parse_ycd(&res).map_err(|err| err.to_string()))
                    {
                        Ok(dict) => {
                            for a in &dict.animations {
                                for f in 0..a.frames {
                                    for k in 0..a.tracks.len() {
                                        let _ = a.value(k, f as f32);
                                    }
                                }
                            }
                            t.textures += dict.clips.len();
                            t.triangles += dict.animations.len();
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if audio {
                // Audio containers have no extension (their hash covers the path).
                if e.ext_id == 0xFF && !e.is_resource {
                    match check_audio(rpf, e) {
                        Ok(Some((sounds, secs))) => {
                            t.textures += sounds;
                            t.triangles += secs.round() as usize;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if meta {
                // .ymap 73, .ytyp 74, .ymt 11, .ymf 1
                if matches!(e.ext_id, 1 | 11 | 73 | 74) {
                    match check_meta(rpf, e) {
                        Ok(problems) => {
                            t.textures += 1;
                            t.triangles += problems;
                            if problems > 0 {
                                t.lines.push(format!("{indent}{name}: {problems} parts not read"));
                            }
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if models {
                if matches!(e.ext_id, 2..=4) && e.is_resource {
                    match check_model(rpf, e) {
                        Ok(n) => {
                            t.textures += 1;
                            t.triangles += n;
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if texts {
                // Extension id 76 is `.#ldb` (`.yldb` on PC).
                if e.ext_id == 76 && e.is_resource {
                    match check_texts(rpf, e) {
                        Ok(n) => {
                            t.textures += 1;
                            t.triangles += n;
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if textures {
                if e.ext_id == 5 && e.is_resource {
                    match check_textures(rpf, e) {
                        Ok(n) => t.textures += n,
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if let Err(err) = rpf.read(e) {
                t.failed += 1;
                t.lines.push(format!("{indent}{name}: {err}"));
            }
        }
    }
    let tallies: Vec<(String, Tally)> = {
        use rayon::prelude::*;
        files
            .par_iter()
            .map(|file| {
                let name = file
                    .strip_prefix(path)
                    .ok()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(file)
                    .display()
                    .to_string();
                let mut t = Tally::default();
                if file.metadata().is_ok_and(|m| m.len() == 0) {
                    t.lines.push(format!("{name}: empty file"));
                    return (name, t);
                }
                match Rpf8::open_with_keys(file, keys.clone()) {
                    Ok(mut rpf) => {
                        if let Some(n) = &names {
                            rpf.set_names(std::sync::Arc::clone(n));
                        }
                        t.archives = 1;
                        let h = rpf.header();
                        let table = if h.is_encrypted() {
                            format!("encrypted (key {})", h.decryption_tag)
                        } else {
                            "plaintext".to_string()
                        };
                        t.lines.push(format!(
                            "{name}: {} entries, table {table}, platform '{}', {} bytes",
                            h.entry_count,
                            h.platform,
                            rpf.size()
                        ));
                        walk(&rpf, 0, list, checks, &mut t);
                    }
                    Err(e) => {
                        t.failed += 1;
                        t.lines.push(format!("{name}: {e}"));
                    }
                }
                (name, t)
            })
            .collect()
    };
    let (mut ok, mut nested, mut plain, mut entries_total, mut failed, mut empty) = (0, 0, 0, 0usize, 0, 0);
    let mut textures_total = 0;
    let mut triangles_total = 0;
    for (_, t) in &tallies {
        for l in &t.lines {
            outln!("{l}");
        }
        if t.archives == 0 && t.failed == 0 {
            empty += 1;
        }
        ok += t.archives;
        nested += t.nested;
        plain += t.readable;
        entries_total += t.entries;
        failed += t.failed;
        textures_total += t.textures;
        triangles_total += t.triangles;
    }
    if textures {
        outln!("\n{textures_total} textures decoded");
    }
    if models {
        outln!("\n{textures_total} models parsed ({triangles_total} triangles)");
    }
    if text {
        outln!("\n{textures_total} text tables read ({triangles_total} strings)");
    }
    if meta {
        outln!("\n{textures_total} metadata files converted ({triangles_total} parts not read)");
    }
    if audio {
        outln!("\n{textures_total} sounds decoded ({:.1} hours)", triangles_total as f64 / 3600.0);
    }
    if anims {
        outln!("\n{textures_total} clips, {triangles_total} animations read (every frame evaluated)");
    }
    outln!(
        "\n{ok} archives read ({nested} more nested in them), {empty} empty, {failed} failed; {plain} with a readable table ({entries_total} entries).{}",
        if keys.is_some() {
            ""
        } else {
            " Encrypted tables need keys from your game: see `openrage rdr2-keys --help`."
        }
    );
    if failed > 0 {
        bail!("{failed} archives or entries could not be read");
    }
    Ok(())
}

/// The stored RDR2 keys, if `openrage rdr2-keys` was run.
pub(crate) fn cmd_rdr2_keys(game: &Path, dump: Option<&Path>, file: Option<&Path>) -> Result<()> {
    use rage_rpf::rpf8_exe::{keys_in_dump, tables_from_exe};
    use rage_rpf::rpf8_keys::{Candidates, archives_under, identify, parse_secrets};
    let started = Instant::now();
    if dump.is_none() && file.is_none() {
        bail!("pass --dump <memory dump of the running game> or --file <key file>");
    }
    let exe_path = game.join(openrage_app::detect::RDR2_EXE);
    let exe = std::fs::read(&exe_path).with_context(|| format!("cannot read {}", exe_path.display()))?;
    let tables = tables_from_exe(&exe)?;
    drop(exe);
    outln!("RDR2.exe: cipher tables and IV found");
    let mut candidates = Candidates { contexts: vec![tables.context], ivs: vec![tables.iv], keys: tables.keys };
    if let Some(file) = file {
        let blocks = parse_secrets(&std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?);
        let before = candidates.keys.len();
        candidates.keys.extend(Candidates::from_blocks(blocks).keys);
        outln!("{}: {} key candidates", file.display(), candidates.keys.len() - before);
    }
    if let Some(dump) = dump {
        let found = keys_in_dump(dump, &tables.key_start, |done, total| {
            eprint!("\rsearching the dump: {} %", done * 100 / total.max(1));
        })
        .with_context(|| format!("cannot read {}", dump.display()))?;
        eprintln!();
        outln!("{}: {} key candidates", dump.display(), found.len());
        candidates.keys.extend(found);
        candidates.keys.sort();
        candidates.keys.dedup();
    }
    let archives = archives_under(game);
    if archives.is_empty() {
        bail!("no .rpf archives under {}", game.display());
    }
    let found = identify(&candidates, &archives)?;
    let path = openrage_app::paths::rdr2_keys().context("no config folder")?;
    found.store.save(&path)?;
    outln!(
        "{} keys recognised; {} of {} encrypted archive tables open; stored in {} ({:.1} s)",
        found.store.keys.len(),
        found.tables.0,
        found.tables.1,
        path.display(),
        started.elapsed().as_secs_f32()
    );
    if !found.missing.is_empty() {
        outln!("Still unknown: key ids {:?} (their files stay closed)", found.missing);
    }
    Ok(())
}

pub(crate) fn cmd_rdr2_ped(
    game: &Path,
    name: Option<&str>,
    outfit: Option<usize>,
    check: bool,
    load: bool,
) -> Result<()> {
    use openrage_app::metaped;
    if !openrage_app::detect::is_rdr2_dir(game) {
        bail!("{} is not a Red Dead Redemption 2 folder", game.display());
    }
    let started = Instant::now();
    let peds = metaped::ped_list(game)?;
    let name = match name {
        Some(n) if !check => n,
        _ if check && load => return load_rdr2_peds(game, &peds, started),
        _ if check => return check_rdr2_peds(game, &peds, started),
        _ => {
            for p in &peds {
                outln!("{:<40} {}", p.name, p.base);
            }
            outln!("{} peds", peds.len());
            return Ok(());
        }
    };
    let finder = PedFinder::new(game)?;
    match peds.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        Some(p) => outln!("{}: base skeleton {}", p.name, metaped::base_fragment(p)),
        None => outln!("{name}: not in the ped list"),
    }
    let outfits = metaped::load_outfits(game, &finder.index, name)?;
    outln!("{} outfits", outfits.len());
    for (k, o) in outfits.iter().enumerate().filter(|(k, _)| outfit.is_none_or(|n| n == *k)) {
        outln!("outfit {k}: {} parts, {} face shape values", o.assets.len(), o.expressions.len());
        for a in &o.assets {
            outln!("  {}", finder.model(a.drawable));
            outln!(
                "    albedo {}  normal {}  material {}",
                finder.texture(a.albedo),
                finder.texture(a.normal),
                rage_formats::names::value(a.material)
            );
            if a.palette != 0 {
                outln!("    palette {}  tints {:?}", finder.palette(a.palette), a.tints);
            }
        }
    }
    eprintln!("{:.2} s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Finds the files of a ped's parts, for showing and checking them.
pub(crate) struct PedFinder {
    pub(crate) index: Arc<openrage_app::metaped::AssetIndex>,
    pub(crate) textures: Arc<openrage_app::rdr2::TextureIndex>,
    pub(crate) palettes: Vec<(u32, String)>,
}

impl PedFinder {
    fn new(game: &Path) -> Result<PedFinder> {
        use openrage_app::metaped;
        let textures = openrage_app::rdr2::texture_index(game)
            .context("no texture index yet: run `openrage rdr2-names <game folder>` first")?;
        let built = std::sync::atomic::AtomicBool::new(false);
        let index = metaped::index(game, |done, total| {
            built.store(true, std::sync::atomic::Ordering::Relaxed);
            if done % 500 == 0 || done == total {
                eprint!("\rindexing the peds' models: {done}/{total} dictionaries");
            }
        })
        .context("no RDR2 keys yet: run `openrage rdr2-keys` first")?;
        if built.into_inner() {
            eprintln!();
        }
        let palettes = metaped::palettes(game)?;
        Ok(PedFinder { index, textures, palettes })
    }

    /// The name of a hash (or the hash), and whether the file was found.
    fn show(hash: u32, found: bool) -> String {
        let name = rage_formats::names::value(hash);
        if found { name } else { format!("{name} (missing)") }
    }

    fn model(&self, hash: u32) -> String {
        match self.index.drawable(hash) {
            Some(d) if hash != 0 => {
                let file = d.file.chain.last().map(|h| rage_formats::names::value(*h)).unwrap_or_default();
                format!("{} (in {file}.ydd #{})", rage_formats::names::value(hash), d.index)
            }
            _ => Self::show(hash, hash == 0),
        }
    }

    fn texture(&self, hash: u32) -> String {
        if hash == 0 {
            return "-".into();
        }
        match self.textures.name_of(hash) {
            Some(n) => n.to_owned(),
            None => Self::show(hash, false),
        }
    }

    fn palette(&self, hash: u32) -> String {
        match self.palettes.iter().find(|(h, _)| *h == hash) {
            Some((_, n)) => n.clone(),
            None => Self::show(hash, false),
        }
    }
}

/// Puts every ped together (its first outfit) as the viewer does: parts,
/// skeleton and textures; reports what fails or stays untextured.
pub(crate) fn load_rdr2_peds(game: &Path, peds: &[openrage_app::metaped::Ped], started: Instant) -> Result<()> {
    use rayon::prelude::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    PedFinder::new(game)?; // builds the index first, once
    let done = AtomicUsize::new(0);
    let failed: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let (parts, shaders, untextured, no_skeleton) =
        (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    // A few at a time: a ped takes a few hundred MB while it is read.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    pool.install(|| {
        peds.par_iter().for_each(|p| {
            match openrage_app::models::load_rdr2_ped(game, &p.name, 0) {
                Ok(m) => {
                    parts.fetch_add(m.parts.len(), Ordering::Relaxed);
                    if m.skeleton.is_none() {
                        no_skeleton.fetch_add(1, Ordering::Relaxed);
                    }
                    for (_, d) in &m.drawables {
                        for s in &d.shaders {
                            shaders.fetch_add(1, Ordering::Relaxed);
                            if s.base_texture().is_none_or(|t| m.texture(d, t).is_none()) {
                                untextured.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
                Err(e) => failed.lock().unwrap().push(format!("{}: {e:#}", p.name)),
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            eprint!("\r{n}/{} peds", peds.len());
        })
    });
    eprintln!();
    let failed = failed.into_inner().unwrap();
    outln!(
        "{} peds put together, {} parts; {} of {} shaders without their colour texture; {} without a skeleton",
        peds.len() - failed.len(),
        parts.into_inner(),
        untextured.into_inner(),
        shaders.into_inner(),
        no_skeleton.into_inner()
    );
    outln!("{} failed", failed.len());
    for f in failed.iter().take(20) {
        outln!("  {f}");
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Checks every ped: its outfit file, every part's model and textures, its palette.
pub(crate) fn check_rdr2_peds(game: &Path, peds: &[openrage_app::metaped::Ped], started: Instant) -> Result<()> {
    use openrage_app::metaped;
    use rage_formats::names::value;
    use std::collections::BTreeMap;
    let finder = PedFinder::new(game)?;
    let (outfit_files, models) = finder.index.counts();
    eprintln!("{outfit_files} outfit files, {models} models indexed ({:.1} s)", started.elapsed().as_secs_f64());
    let (mut outfits, mut parts) = (0usize, 0usize);
    // Problem → (count, example).
    let mut missing: BTreeMap<&'static str, (usize, String)> = BTreeMap::new();
    let mut note = |what: &'static str, example: String| {
        missing.entry(what).or_insert((0, example)).0 += 1;
    };
    for (n, p) in peds.iter().enumerate() {
        eprint!("\r{}/{} peds", n + 1, peds.len());
        let list = match metaped::load_outfits(game, &finder.index, &p.name) {
            Ok(l) => l,
            Err(e) => {
                note("outfit file", format!("{}: {e:#}", p.name));
                continue;
            }
        };
        if list.is_empty() {
            note("ped without outfits", p.name.clone());
        }
        outfits += list.len();
        for o in &list {
            for a in &o.assets {
                parts += 1;
                if a.drawable == 0 {
                    note("part without model", p.name.clone());
                } else if finder.index.drawable(a.drawable).is_none() {
                    note("model", format!("{} ({})", value(a.drawable), p.name));
                }
                for (hash, what) in [(a.albedo, "albedo texture"), (a.normal, "normal texture")] {
                    if hash != 0 && finder.textures.find_hash(hash).is_none() {
                        note(what, format!("{} ({})", value(hash), p.name));
                    }
                }
                if a.palette != 0 && !finder.palettes.iter().any(|(h, _)| *h == a.palette) {
                    note("palette", format!("{} ({})", value(a.palette), p.name));
                }
            }
        }
    }
    eprintln!();
    outln!("{} peds, {outfits} outfits, {parts} parts", peds.len());
    if missing.is_empty() {
        outln!("all parts found");
    }
    for (what, (count, example)) in &missing {
        outln!("missing {what}: {count} (e.g. {example})");
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

pub(crate) fn cmd_rdr2_names(game: &Path, import: Option<&Path>) -> Result<()> {
    use rage_rpf::rpf8_keys::archives_under;
    use rage_rpf::rpf8_names::{NameIndex, build, entry_hashes};
    let started = Instant::now();
    let keys = openrage_app::rdr2::load_keys()?.context("no RDR2 keys yet: run `openrage rdr2-keys` first")?;
    let files = archives_under(game);
    if files.is_empty() {
        bail!("no .rpf archives under {}", game.display());
    }
    let wanted = entry_hashes(&files, &keys);
    let path = openrage_app::paths::rdr2_names(game).context("no cache folder")?;
    let mut index = NameIndex::load(&path)?.unwrap_or_default();
    match import {
        Some(list) => {
            let text = std::fs::read_to_string(list).with_context(|| format!("cannot read {}", list.display()))?;
            let added = index.add_list(&text, &wanted);
            outln!("{}: {added} new names", list.display());
        }
        None => {
            let found = build(&files, &keys, &wanted, |done, total| {
                eprint!("\rsearching the game's files: {done}/{total} archives")
            });
            eprintln!();
            for (h, n) in found.names {
                let files = found.files.get(&h).copied().unwrap_or(1);
                index.files.entry(h).or_insert(files);
                index.names.entry(h).or_insert(n);
            }
        }
    }
    index.save(&path)?;
    if import.is_none() {
        let textures = openrage_app::rdr2::build_texture_index(game, &files, &keys, |done, total| {
            eprint!("\rindexing textures: {done}/{total} archives")
        });
        eprintln!();
        let tpath = openrage_app::rdr2::TextureIndex::default_path(game).context("no cache folder")?;
        textures.save(&tpath)?;
        outln!(
            "{} textures in {} dictionaries indexed; stored in {}",
            textures.textures.len(),
            textures.dictionaries.len(),
            tpath.display()
        );
    }
    outln!(
        "{} of {} entry names known ({:.1} %); stored in {} ({:.0} s)",
        index.trusted_count(),
        wanted.len(),
        index.trusted_count() as f64 * 100.0 / wanted.len().max(1) as f64,
        path.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
