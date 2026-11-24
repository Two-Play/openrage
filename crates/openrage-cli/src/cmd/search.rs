//! The search index: `index`, `search`, `find`.

use crate::*;

pub(crate) fn open_index(game_dir: &Path) -> Result<rage_index::Index> {
    let path = openrage_app::index_path(game_dir).context("no cache directory available")?;
    Ok(rage_index::Index::open(&path)?)
}

pub(crate) fn cmd_index(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let mut index = open_index(game_dir)?;
    // A GTA VI archive reports each part of it: not a line for every one.
    let mut last = (0, Instant::now());
    let stats = index.update(game_dir, keys, &AtomicBool::new(false), &mut |p| {
        if p.archives_done == last.0 && last.1.elapsed().as_secs() < 2 {
            return;
        }
        last = (p.archives_done, Instant::now());
        eprintln!("  [{}/{}] {} ({} entries)", p.archives_done, p.archives_total, p.current, p.entries);
    })?;
    let names = openrage_app::rpf9::learn_names(&mut index)?;
    if names > 0 {
        outln!("Learned {names} names of GTA VI files from other archives.");
    }
    let s = index.stats()?;
    outln!(
        "Indexed {} archives, removed {}, unchanged {} in {:.1}s ({} read errors).",
        stats.archives_indexed,
        stats.archives_removed,
        stats.archives_unchanged,
        stats.seconds,
        stats.read_errors
    );
    outln!(
        "Index: {} archives, {} entries, {} text documents, {} dictionary strings, {} on disk ({})",
        s.archives,
        s.entries,
        s.text_documents,
        s.dictionary,
        human(s.file_bytes),
        index.path().display()
    );
    Ok(())
}

pub(crate) fn cmd_search(
    game_dir: &Path,
    query: &str,
    limit: usize,
    lang: Option<&str>,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    if let Some(l) = lang.filter(|l| !rage_index::LANGUAGES.contains(l)) {
        bail!("unknown language {l:?}; use one of {} or `all`", rage_index::LANGUAGES.join(", "));
    }
    let index = open_index(game_dir)?;
    if index.stats()?.updated_at.is_none() {
        bail!("the search index has not been built yet; run `openrage index` first");
    }
    let started = Instant::now();
    let results = index.search(query, limit, lang)?;
    let elapsed = started.elapsed();
    let reason = |r: &rage_index::HitReason| match r {
        rage_index::HitReason::Name => "name",
        rage_index::HitReason::NameHash => "hash",
        rage_index::HitReason::Content => "text",
    };
    let needle = results.hash.map(|h| format!("0x{h:08X}")).unwrap_or_else(|| query.to_owned());
    if json() {
        let hits: Vec<_> = results
            .hits
            .iter()
            .map(|hit| {
                let snippet = (hit.reason == rage_index::HitReason::Content)
                    .then(|| content_snippet(game_dir, &hit.vpath, &needle, keys.clone()))
                    .flatten();
                serde_json::json!({
                    "match": reason(&hit.reason),
                    "size": hit.size,
                    "path": hit.vpath,
                    "line": snippet,
                })
            })
            .collect();
        let value = serde_json::json!({
            "query": query,
            "query_hash": format!("0x{:08X}", results.query_hash),
            "hash": results.hash.map(|h| format!("0x{h:08X}")),
            "hash_strings": results.dictionary,
            "hits": hits,
            "limited": results.limited,
        });
        outln!("{value:#}");
        return Ok(());
    }
    match results.hash {
        Some(hash) => {
            outln!("Hash 0x{hash:08X} = {} (signed {})", hash, hash as i32);
            if results.dictionary.is_empty() {
                outln!("  no known string for this hash");
            }
            for s in &results.dictionary {
                outln!("  = \"{s}\"");
            }
        }
        None => outln!("joaat(\"{query}\") = 0x{:08X}", results.query_hash),
    }
    for hit in &results.hits {
        outln!("{:<5} {:>8}  {}", reason(&hit.reason), human(hit.size), hit.vpath);
        if hit.reason == rage_index::HitReason::Content
            && let Some(line) = content_snippet(game_dir, &hit.vpath, &needle, keys.clone())
        {
            outln!("      └ {line}");
        }
    }
    eprintln!(
        "{} hits{} in {:.0} ms",
        results.hits.len(),
        if results.limited { " (limited, use -n for more)" } else { "" },
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

/// Matching line of an indexed text document, read back from the archive.
pub(crate) fn content_snippet(
    game_dir: &Path,
    vpath: &str,
    query: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Option<String> {
    let text = rage_index::node_text(&vfs::resolve(game_dir, vpath, keys).ok()?)?;
    rage_index::snippet(&text, query)
}

pub(crate) fn cmd_find(game_dir: &Path, within: &str, pattern: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let needle = pattern.to_ascii_lowercase();
    match vfs::resolve(game_dir, within, keys.clone())? {
        Node::Rpf9Dir(rpf, dir) => {
            let prefix = within.trim_end_matches('/');
            let top = if dir.is_empty() { prefix } else { prefix[..prefix.len() - dir.len()].trim_end_matches('/') };
            return find_rpf9(&[(top.to_owned(), rpf, dir)], &needle);
        }
        Node::Dir(dir) if openrage_app::detect::is_gta6_dir(game_dir) => {
            let mut found = Vec::new();
            collect_rpfs(&dir, &mut found);
            let mut starts = Vec::new();
            for path in found.into_iter().filter(|p| rage_rpf::Rpf9::file_is_rpf9(p)) {
                let rel = path.strip_prefix(game_dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                // Asked one after the other: the first loads the keys.
                match rage_rpf::Rpf9::open(&path) {
                    Ok(rpf) if rpf.is_native() => starts.push((rel, Arc::new(rpf), String::new())),
                    Ok(_) => eprintln!("warning: {rel}: its table cannot be read"),
                    Err(e) => eprintln!("warning: {e}"),
                }
            }
            return find_rpf9(&starts, &needle);
        }
        _ => {}
    }
    let starts = archive_roots(game_dir, within, keys)?;

    let started = Instant::now();
    let results: Vec<(Vec<String>, usize)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut hits = Vec::new();
            let mut scanned = 0usize;
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    scanned += 1;
                    if a.entry(id).name.to_ascii_lowercase().contains(&needle) {
                        hits.push(p.to_owned());
                    }
                },
                &mut |e| eprintln!("warning: {e}"),
            );
            (hits, scanned)
        })
        .collect();

    let mut total = 0;
    let mut hits = 0;
    for (h, n) in results {
        total += n;
        hits += h.len();
        h.iter().for_each(|p| outln!("{p}"));
    }
    eprintln!("{hits} matches in {total} entries ({:.1}s)", started.elapsed().as_secs_f32());
    Ok(())
}

/// `find` in GTA VI archives and those inside them, below a folder of each.
fn find_rpf9(starts: &[(String, Arc<rage_rpf::Rpf9>, String)], needle: &str) -> Result<()> {
    fn walk(prefix: &str, rpf: &rage_rpf::Rpf9, dir: &str, needle: &str) -> (usize, usize) {
        let entries = match rpf.entries() {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("warning: {e}");
                return (0, 0);
            }
        };
        let below = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        entries
            .par_iter()
            .map(|e| {
                let name = rpf.entry_name(e);
                if !name.get(..below.len()).is_some_and(|start| start.eq_ignore_ascii_case(&below)) {
                    return (0, 0);
                }
                let path = format!("{prefix}/{name}");
                let base = name.rsplit('/').next().unwrap_or(&name);
                let mut found = (0, 1);
                if base.to_ascii_lowercase().contains(needle) {
                    outln!("{path}");
                    found.0 = 1;
                }
                if rpf.is_archive(e) {
                    match rpf.open_child(e) {
                        Ok(child) => {
                            let inside = walk(&path, &child, "", needle);
                            found = (found.0 + inside.0, found.1 + inside.1);
                        }
                        Err(e) => eprintln!("warning: {e}"),
                    }
                }
                found
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }
    let started = Instant::now();
    let (hits, total) = starts
        .iter()
        .map(|(prefix, rpf, dir)| walk(prefix, rpf, dir, needle))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    eprintln!("{hits} matches in {total} entries ({:.1}s)", started.elapsed().as_secs_f32());
    Ok(())
}
