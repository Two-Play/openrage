//! The search index: `index`, `search`, `find`.

use crate::*;

pub(crate) fn open_index(game_dir: &Path) -> Result<rage_index::Index> {
    let path = openrage_app::index_path(game_dir).context("no cache directory available")?;
    Ok(rage_index::Index::open(&path)?)
}

pub(crate) fn cmd_index(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let mut index = open_index(game_dir)?;
    let stats = index.update(game_dir, keys, &AtomicBool::new(false), &mut |p| {
        eprintln!("  [{}/{}] {} ({} entries)", p.archives_done, p.archives_total, p.current, p.entries);
    })?;
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
        let tag = match hit.reason {
            rage_index::HitReason::Name => "name",
            rage_index::HitReason::NameHash => "hash",
            rage_index::HitReason::Content => "text",
        };
        outln!("{tag:<5} {:>8}  {}", human(hit.size), hit.vpath);
        if hit.reason == rage_index::HitReason::Content {
            let needle = results.hash.map(|h| format!("0x{h:08X}")).unwrap_or_else(|| query.to_owned());
            if let Some(line) = content_snippet(game_dir, &hit.vpath, &hit.name, &needle, keys.clone()) {
                outln!("      └ {line}");
            }
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
    name: &str,
    query: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Option<String> {
    let Node::ArchiveFile(rpf, id) = vfs::resolve(game_dir, vpath, keys).ok()? else { return None };
    let data = rpf.read(id).ok()?;
    let (text, _) = rage_index::content_for(name, &data)?;
    rage_index::snippet(&text, query)
}

pub(crate) fn cmd_find(game_dir: &Path, within: &str, pattern: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let needle = pattern.to_ascii_lowercase();
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
