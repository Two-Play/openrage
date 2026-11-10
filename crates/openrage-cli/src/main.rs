//! `openrage` — command-line access to RAGE game archives.

/// `println!` that exits quietly when stdout is closed (e.g. piped into `head`).
macro_rules! outln {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        if writeln!(std::io::stdout().lock(), $($t)*).is_err() {
            std::process::exit(0);
        }
    }};
}

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use openrage_app::config::Config;
use openrage_app::extract::{extract_entry, human_size as human};
use openrage_app::setup::{self, KeySource};
use rage_crypto::GtaVKeys;
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, Rpf7};
use rayon::prelude::*;

#[derive(Parser)]
#[command(name = "openrage", version, about = "Open-source explorer for RAGE engine game files")]
struct Cli {
    /// Game installation directory (remembered after `openrage keys`).
    #[arg(long, global = true, env = "OPENRAGE_GAME_DIR")]
    game: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derive the archive keys from your own game installation and cache them locally.
    Keys {
        /// Game executable to scan (default: GTA5_Enhanced.exe or GTA5.exe in the game directory).
        #[arg(long)]
        exe: Option<PathBuf>,
        /// CodeWalker key blob (magic.dat); required for GTA V Enhanced, whose
        /// executable no longer contains the NG keys in plain form.
        #[arg(long)]
        magic: Option<PathBuf>,
    },
    /// List a directory, an archive, or a directory inside an archive.
    Ls {
        /// Virtual path relative to the game directory, e.g. `x64a.rpf/data`.
        #[arg(default_value = "")]
        path: String,
        /// Recurse into subdirectories and nested archives.
        #[arg(short, long)]
        recursive: bool,
    },
    /// Show details about an archive or an entry.
    Info { path: String },
    /// Extract a file or a whole directory/archive.
    Extract {
        path: String,
        /// Output directory.
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// Unpack nested archives into directories instead of writing them as .rpf files.
        #[arg(long)]
        expand_archives: bool,
    },
    /// Read and decode every entry to check that all archives can be processed.
    Verify {
        /// Limit the check to this virtual path.
        #[arg(default_value = "")]
        path: String,
    },
    /// Build or update the search index (only changed archives are re-read).
    Index,
    /// Search the index: names, hashes (`0xB779A091`, decimal) and file contents.
    Search {
        query: String,
        /// Maximum number of hits.
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        /// Only show game text in this language (`german`, `american`, …, or `all`).
        /// Defaults to the language of your system locale.
        #[arg(long)]
        lang: Option<String>,
    },
    /// Search all archives of the game for entries whose name contains PATTERN.
    Find {
        pattern: String,
        /// Limit the search to this virtual path.
        #[arg(long, default_value = "")]
        r#in: String,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = Config::load();
    let game_dir = cli.game.clone().or_else(|| cfg.game_dir.clone());

    if let Command::Keys { exe, magic } = &cli.command {
        let game_dir = game_dir.context("no game directory given; pass --game <DIR>")?;
        let magic = magic.clone().or_else(|| cfg.magic_path.clone());
        cmd_keys(&game_dir, exe.as_deref(), magic.as_deref())?;
        cfg.game_dir = Some(game_dir);
        cfg.magic_path = magic;
        cfg.save()?;
        return Ok(());
    }

    let game_dir = game_dir.context("no game directory configured; run `openrage --game <DIR> keys` first")?;
    let keys = openrage_app::load_keys().unwrap_or_else(|e| {
        eprintln!("warning: {e}");
        None
    });

    match cli.command {
        Command::Keys { .. } => unreachable!(),
        Command::Ls { path, recursive } => cmd_ls(&game_dir, &path, recursive, keys),
        Command::Info { path } => cmd_info(&game_dir, &path, keys),
        Command::Extract { path, out, expand_archives } => cmd_extract(&game_dir, &path, &out, expand_archives, keys),
        Command::Find { pattern, r#in } => cmd_find(&game_dir, &r#in, &pattern, keys),
        Command::Verify { path } => cmd_verify(&game_dir, &path, keys),
        Command::Index => cmd_index(&game_dir, keys),
        Command::Search { query, limit, lang } => {
            let lang = match lang.as_deref() {
                Some("all") => None,
                Some(l) => Some(l.to_ascii_lowercase()),
                None => openrage_app::game_language_from_locale().map(str::to_owned),
            };
            cmd_search(&game_dir, &query, limit, lang.as_deref(), keys)
        }
    }
}

fn cmd_keys(game_dir: &Path, exe: Option<&Path>, magic: Option<&Path>) -> Result<()> {
    let (_, report) = setup::derive_keys(game_dir, exe, magic, &mut |msg| outln!("{msg}"))?;
    outln!(
        "  scanned in {:.1}s; NG keys in executable: {}/101, NG tables: {}/272",
        report.scan_seconds,
        report.ng_keys_in_exe,
        report.ng_tables_in_exe
    );
    match report.source {
        KeySource::Executable => outln!("Complete key set found in the executable."),
        KeySource::KeyBlob => outln!("NG keys decoded from the key blob."),
    }
    if let Some(n) = report.verified_entries {
        outln!("Verified: common.rpf decrypts ({n} entries).");
    }
    outln!("Keys cached at {} (local only, never distribute this file).", report.cache_path.display());
    Ok(())
}

fn print_entry(rpf: &Rpf7, id: EntryId, path: &str) {
    let e = rpf.entry(id);
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    let suffix = if e.is_dir() || e.is_archive() { "/" } else { "" };
    outln!("{:<8} {:>8} {:>4}  {path}{suffix}", e.kind_label(), human(e.size()), version);
}

fn cmd_ls(game_dir: &Path, path: &str, recursive: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::Dir(dir) => {
            let mut items: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
            items.sort_by_key(|e| e.file_name());
            for item in items {
                let meta = item.metadata()?;
                let name = item.file_name().to_string_lossy().into_owned();
                let kind = if meta.is_dir() {
                    "dir"
                } else if name.to_ascii_lowercase().ends_with(".rpf") {
                    "rpf"
                } else {
                    "file"
                };
                let slash = if kind == "file" { "" } else { "/" };
                outln!("{kind:<8} {:>8}       {name}{slash}", human(meta.len()));
            }
        }
        Node::PlainFile(p) => outln!("file     {:>8}       {}", human(fs::metadata(&p)?.len()), p.display()),
        Node::ArchiveFile(rpf, id) => print_entry(&rpf, id, &rpf.path_of(id)),
        Node::ArchiveDir(rpf, dir) => {
            if recursive {
                vfs::walk(&rpf, dir, "", true, &mut |a, id, p| print_entry(a, id, p), &mut |e| {
                    eprintln!("warning: {e}")
                });
            } else {
                for id in rpf.children(dir) {
                    print_entry(&rpf, id, &rpf.entry(id).name);
                }
            }
        }
    }
    Ok(())
}

fn cmd_info(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveDir(rpf, dir) if dir == Rpf7::ROOT => {
            let (mut dirs, mut files, mut resources, mut archives) = (0, 0, 0, 0);
            for e in rpf.entries() {
                match e.kind_label() {
                    "dir" => dirs += 1,
                    "rpf" => archives += 1,
                    "resource" => resources += 1,
                    _ => files += 1,
                }
            }
            outln!("Archive:     {}", rpf.display_path());
            outln!("Format:      RPF7");
            outln!("Size:        {} ({} bytes)", human(rpf.size()), rpf.size());
            outln!("Encryption:  {:?}", rpf.encryption());
            outln!(
                "Entries:     {} ({dirs} dirs, {files} files, {resources} resources, {archives} archives)",
                rpf.entries().len()
            );
        }
        Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id) => {
            let e = rpf.entry(id);
            outln!("Path:        {}/{}", rpf.display_path(), rpf.path_of(id));
            outln!("Kind:        {}", e.kind_label());
            outln!("Details:     {:?}", e.kind);
            if let Some(v) = e.resource_version() {
                outln!("Resource version: {v}");
            }
        }
        Node::Dir(p) | Node::PlainFile(p) => outln!("{} is not inside an archive", p.display()),
    }
    Ok(())
}

fn cmd_extract(game_dir: &Path, path: &str, out: &Path, expand: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let (rpf, id) = match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id) => (rpf, id),
        Node::Dir(_) | Node::PlainFile(_) => bail!("{path} is not inside an archive"),
    };
    let summary = extract_entry(&rpf, id, out, expand, &AtomicBool::new(false), &mut |_, _| {})?;
    for failure in &summary.failures {
        eprintln!("warning: {failure}");
    }
    outln!(
        "Extracted {} files ({}) to {}{}",
        summary.files,
        human(summary.bytes),
        summary.target.display(),
        if summary.failures.is_empty() { String::new() } else { format!(" ({} failures)", summary.failures.len()) }
    );
    Ok(())
}

fn open_index(game_dir: &Path) -> Result<rage_index::Index> {
    let path = openrage_app::index_path(game_dir).context("no cache directory available")?;
    Ok(rage_index::Index::open(&path)?)
}

fn cmd_index(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
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

fn cmd_search(
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
fn content_snippet(
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

fn cmd_find(game_dir: &Path, within: &str, pattern: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
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

type Start = (String, Arc<Rpf7>, EntryId);

/// Resolves `within` to the archive directories to process: either one
/// directory inside an archive, or every `.rpf` below a real directory.
fn archive_roots(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<Vec<Start>> {
    Ok(match vfs::resolve(game_dir, within, keys.clone())? {
        Node::ArchiveDir(rpf, dir) => vec![(within.trim_end_matches('/').to_owned(), rpf, dir)],
        Node::ArchiveFile(..) | Node::PlainFile(_) => bail!("{within} is not a directory"),
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
struct VerifyStats {
    archives: usize,
    files: usize,
    resources: usize,
    bytes: u64,
    errors: Vec<String>,
}

fn cmd_verify(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
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

fn collect_rpfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            collect_rpfs(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("rpf")) {
            out.push(p);
        }
    }
}
