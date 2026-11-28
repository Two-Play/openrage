//! Browsing and extracting archives, keys and licences.

use crate::*;

mod info;
pub(crate) use info::cmd_info;

pub(crate) fn cmd_gta4_keys(game_dir: &Path) -> Result<()> {
    let (_, report) = openrage_app::gta4::derive_key(game_dir)?;
    outln!("GTA IV archive key found in {} ({:.1}s).", report.exe.display(), report.scan_seconds);
    if let Some(n) = report.verified_entries {
        outln!("Verified: vehicles.img decrypts ({n} entries).");
    }
    outln!("Key cached at {} (local only, never distribute this file).", report.cache_path.display());
    Ok(())
}

/// One row of `openrage ls`: the text line and its JSON form.
fn ls_row(out: &mut Rows, kind: &str, size: Option<u64>, version: &str, path: &str, folder: bool) {
    let slash = if folder { "/" } else { "" };
    out.row(
        || format!("{kind:<8} {:>8} {version:>4}  {path}{slash}", size.map(human).unwrap_or_default()),
        serde_json::json!({ "kind": kind, "size": size, "version": version, "path": path, "folder": folder }),
    );
}

pub(crate) fn print_gta4_entry(out: &mut Rows, archive: &rage_rpf::Gta4Archive, id: rage_rpf::Gta4EntryId, path: &str) {
    let e = archive.entry(id);
    let kind = openrage_app::gta4::kind_label(archive, id);
    let ty = e.resource_type().map(|t| format!("t{t}")).unwrap_or_default();
    ls_row(out, kind, Some(e.size()), &ty, path, e.is_dir());
}

pub(crate) fn cmd_licenses(text: Option<&str>, all: bool) -> Result<()> {
    use openrage_app::licenses;
    match text {
        Some(name) if name.eq_ignore_ascii_case("openrage") => outln!("{}", licenses::gpl_text()),
        Some(name) if name.eq_ignore_ascii_case("notice") => outln!("{}", licenses::notice()),
        Some(name) => {
            let found: Vec<_> = licenses::dependencies().iter().filter(|d| d.name.eq_ignore_ascii_case(name)).collect();
            if found.is_empty() {
                bail!("no crate named {name}; `openrage licenses` lists them");
            }
            for d in found {
                outln!(
                    "{} {} — {}
",
                    d.name,
                    d.version,
                    d.license
                );
                outln!(
                    "{}
",
                    licenses::dependency_text(d)
                );
            }
        }
        None if all => {
            for d in licenses::dependencies() {
                outln!(
                    "==== {} {} — {}
",
                    d.name,
                    d.version,
                    d.license
                );
                outln!(
                    "{}
",
                    licenses::dependency_text(d)
                );
            }
        }
        None => {
            outln!("OpenRAGE {} — {} ({})", licenses::VERSION, licenses::LICENSE, licenses::REPOSITORY);
            outln!(
                "{}
",
                licenses::NOTICE_SHORT
            );
            outln!("Credits (NOTICE.md):");
            for c in licenses::credits() {
                outln!("  {}", c.title);
            }
            let deps = licenses::dependencies();
            outln!(
                "
{} third-party crates:",
                deps.len()
            );
            for d in deps {
                outln!("  {:<36} {:<12} {}", d.name, d.version, d.license);
            }
            outln!(
                "
Full texts: `openrage licenses --text <crate>` (`openrage` for the GPL, `notice` for NOTICE.md) or `--all`."
            );
        }
    }
    Ok(())
}

pub(crate) fn cmd_keys(game_dir: &Path, exe: Option<&Path>, magic: Option<&Path>) -> Result<()> {
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

pub(crate) fn print_entry(out: &mut Rows, rpf: &Rpf7, id: EntryId, path: &str) {
    let e = rpf.entry(id);
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    ls_row(out, e.kind_label(), Some(e.size()), &version, path, e.is_dir() || e.is_archive());
}

pub(crate) fn cmd_ls(game_dir: &Path, path: &str, recursive: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let node = vfs::resolve(game_dir, path, keys)?;
    let mut out = Rows::start();
    match node {
        node @ (Node::Dir(..) | Node::PlainFile(..)) => ls_disk(&mut out, node)?,
        node @ (Node::ArchiveFile(..) | Node::ArchiveDir(..)) => ls_rpf7(&mut out, node, recursive),
        node @ (Node::Rpf8File(..) | Node::Rpf8Dir(..)) => ls_rpf8(&mut out, node),
        node @ (Node::Rpf9File(..) | Node::Rpf9Dir(..)) => ls_rpf9(&mut out, node, recursive)?,
        node @ (Node::Gta4File(..) | Node::Gta4Dir(..)) => ls_gta4(&mut out, node, recursive),
        _ => bail!("{path}: not supported"),
    }
    out.finish();
    Ok(())
}

fn ls_disk(out: &mut Rows, node: Node) -> Result<()> {
    match node {
        Node::Dir(dir) => ls_disk_dir(out, &dir)?,
        Node::PlainFile(p) => ls_row(out, "file", Some(fs::metadata(&p)?.len()), "", &p.display().to_string(), false),
        _ => {}
    }
    Ok(())
}

fn ls_rpf7(out: &mut Rows, node: Node, recursive: bool) {
    match node {
        Node::ArchiveFile(rpf, id) => print_entry(out, &rpf, id, &rpf.path_of(id)),
        Node::ArchiveDir(rpf, dir) => ls_archive_dir(out, &rpf, dir, recursive),
        _ => {}
    }
}

fn ls_rpf8(out: &mut Rows, node: Node) {
    match node {
        Node::Rpf8File(rpf, i) => print_rpf8_entry(out, &rpf, i, &rpf.entry_name(i)),
        Node::Rpf8Dir(rpf, dir) => ls_rpf8_dir(out, &rpf, &dir),
        _ => {}
    }
}

fn ls_rpf9(out: &mut Rows, node: Node, recursive: bool) -> Result<()> {
    match node {
        Node::Rpf9File(rpf, i) => {
            let entries = rpf.entries()?;
            print_rpf9_entry(out, &rpf, &entries[i], &rpf.entry_name(&entries[i]));
            Ok(())
        }
        Node::Rpf9Dir(rpf, dir) => ls_rpf9_dir(out, &rpf, &dir, recursive),
        _ => Ok(()),
    }
}

fn ls_gta4(out: &mut Rows, node: Node, recursive: bool) {
    match node {
        Node::Gta4File(a, id) => print_gta4_entry(out, &a, id, &a.entry_path(id)),
        Node::Gta4Dir(a, dir) => ls_gta4_dir(out, &a, dir, recursive),
        _ => {}
    }
}

fn ls_disk_dir(out: &mut Rows, dir: &Path) -> Result<()> {
    let mut items: Vec<_> = fs::read_dir(dir)?.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let meta = item.metadata()?;
        let name = item.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let kind = if meta.is_dir() {
            "dir"
        } else if lower.ends_with(".rpf") {
            "rpf"
        } else if lower.ends_with(".img") && rage_rpf::Gta4Archive::file_is_gta4(&item.path()) {
            "img"
        } else {
            "file"
        };
        ls_row(out, kind, Some(meta.len()), "", &name, kind != "file");
    }
    Ok(())
}

fn ls_rpf8_dir(out: &mut Rows, rpf: &rage_rpf::Rpf8, dir: &str) {
    let (dirs, files) = rpf.list_dir(dir);
    for d in dirs {
        ls_row(out, "dir", None, "", &d, true);
    }
    for i in files {
        let name = rpf.entry_name(i);
        print_rpf8_entry(out, rpf, i, name.rsplit('/').next().unwrap_or(&name));
    }
}

fn ls_rpf9_dir(out: &mut Rows, rpf: &rage_rpf::Rpf9, dir: &str, recursive: bool) -> Result<()> {
    let entries = rpf.entries()?;
    openrage_app::rpf9::names_now(rpf);
    if recursive {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        for e in entries.iter() {
            let name = rpf.entry_name(e);
            let below = name.get(..prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(&prefix));
            if let Some(rel) = name.get(prefix.len()..).filter(|_| below) {
                print_rpf9_entry(out, rpf, e, rel);
            }
        }
    } else {
        let (dirs, files) = rpf.list_dir(&entries, dir);
        for d in dirs {
            ls_row(out, "dir", None, "", &d, true);
        }
        for i in files {
            let name = rpf.entry_name(&entries[i]);
            print_rpf9_entry(out, rpf, &entries[i], name.rsplit('/').next().unwrap_or(&name));
        }
    }
    Ok(())
}

fn ls_gta4_dir(out: &mut Rows, a: &rage_rpf::Gta4Archive, dir: rage_rpf::Gta4EntryId, recursive: bool) {
    if recursive {
        vfs::walk_gta4(a, dir, "", &mut |id, p| print_gta4_entry(out, a, id, p));
    } else {
        for id in a.children(dir) {
            print_gta4_entry(out, a, id, &a.entry(id).name);
        }
    }
}

fn ls_archive_dir(out: &mut Rows, rpf: &Rpf7, dir: EntryId, recursive: bool) {
    if recursive {
        vfs::walk(rpf, dir, "", true, &mut |a, id, p| print_entry(out, a, id, p), &mut |e| eprintln!("warning: {e}"));
    } else {
        for id in rpf.children(dir) {
            print_entry(out, rpf, id, &rpf.entry(id).name);
        }
    }
}

pub(crate) fn report_rpf8_extract(rpf: &rage_rpf::Rpf8, dir: &str, file: Option<usize>, out: &Path) -> Result<()> {
    let s = openrage_app::extract::extract_rpf8(rpf, dir, file, out, &AtomicBool::new(false), &mut |_, _| {})?;
    for f in &s.failures {
        eprintln!("warning: {f}");
    }
    outln!("{} file(s) ({}) extracted to {}, {} failed", s.files, human(s.bytes), out.display(), s.failures.len());
    extraction_result(s.failures.len())
}

pub(crate) fn report_rpf9_extract(
    rpf: &rage_rpf::Rpf9,
    dir: &str,
    file: Option<usize>,
    out: &Path,
    expand: bool,
) -> Result<()> {
    let s = openrage_app::rpf9::extract(rpf, dir, file, out, expand, &AtomicBool::new(false), &mut |_, _| {})?;
    for f in &s.failures {
        eprintln!("warning: {f}");
    }
    outln!("{} file(s) ({}) extracted to {}, {} failed", s.files, human(s.bytes), out.display(), s.failures.len());
    extraction_result(s.failures.len())
}

/// One file of a GTA VI archive.
pub(crate) fn print_rpf9_entry(out: &mut Rows, rpf: &rage_rpf::Rpf9, e: &rage_rpf::rpf9::Entry, name: &str) {
    let archive = rpf.is_archive(e);
    let kind = if archive {
        "rpf"
    } else if rpf.resource_parts(e).is_some() {
        "resource"
    } else {
        "file"
    };
    ls_row(out, kind, Some(e.size), "", name, archive);
}

/// Extraction goes on past single failures, but the command still fails then,
/// so scripts notice.
pub(crate) fn extraction_result(failures: usize) -> Result<()> {
    if failures > 0 {
        bail!("{failures} file(s) could not be extracted (see the warnings above)");
    }
    Ok(())
}

pub(crate) fn print_rpf8_entry(out: &mut Rows, rpf: &rage_rpf::Rpf8, i: usize, name: &str) {
    let Some(e) = rpf.entries().and_then(|all| all.get(i)) else { return };
    let kind = if e.ext_id == 0 {
        "rpf"
    } else if e.is_resource {
        "resource"
    } else {
        "file"
    };
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    ls_row(out, kind, Some(e.size()), &version, name, e.ext_id == 0);
}

pub(crate) fn cmd_extract(
    game_dir: &Path,
    path: &str,
    out: &Path,
    expand: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let (rpf, id) = match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id) => (rpf, id),
        Node::Rpf8File(rpf, i) => return report_rpf8_extract(&rpf, "", Some(i), out),
        Node::Rpf8Dir(rpf, dir) => return report_rpf8_extract(&rpf, &dir, None, out),
        Node::Rpf9File(rpf, i) => return report_rpf9_extract(&rpf, "", Some(i), out, expand),
        Node::Rpf9Dir(rpf, dir) => return report_rpf9_extract(&rpf, &dir, None, out, expand),
        Node::Gta4Dir(a, id) | Node::Gta4File(a, id) => {
            let s = openrage_app::gta4::extract(&a, id, out, &AtomicBool::new(false), &mut |_, _| {})?;
            for f in &s.failures {
                eprintln!("warning: {f}");
            }
            outln!(
                "Extracted {} files ({}) to {}, {} failed",
                s.files,
                human(s.bytes),
                s.target.display(),
                s.failures.len()
            );
            return extraction_result(s.failures.len());
        }
        _ => bail!("{path} is not inside an archive"),
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
    extraction_result(summary.failures.len())
}

pub(crate) fn cmd_mods(game_dir: &Path, changes: bool, deep: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let Some(mods) = rage_mods::mods_dir(game_dir) else {
        outln!("No mods folder in {}.", game_dir.display());
        return Ok(());
    };
    let files = rage_mods::scan(game_dir);
    outln!("{} files in {}", files.len(), mods.display());
    for f in &files {
        let kind = match f.kind {
            rage_mods::ModFileKind::Replaces => "replaces",
            rage_mods::ModFileKind::New => "new",
        };
        outln!("{kind:<9} {:>8}  {}", human(f.size), f.rel);
    }
    if !changes {
        return Ok(());
    }
    let mode = if deep { rage_mods::CompareMode::Deep } else { rage_mods::CompareMode::Quick };
    let started = Instant::now();
    let list =
        rage_mods::list_changes(game_dir, keys, mode, &mut |i, n, p| eprintln!("  [{}/{n}] comparing {p}", i + 1));
    outln!();
    for c in &list {
        let slash = if c.is_dir { "/" } else { "" };
        outln!("{:<15} {:>8}  {}{slash}", c.kind.label(), human(c.size), c.vpath);
    }
    let in_game = rage_mods::list_game_changes(game_dir);
    if !in_game.is_empty() {
        outln!("\nIn the game folder itself (originals kept, `openrage originals`):");
        for c in &in_game {
            outln!("{:<15} {:>8}  {}", c.kind.label(), human(c.size), c.vpath);
        }
    }
    eprintln!("{} changes ({:.1}s)", list.len() + in_game.len(), started.elapsed().as_secs_f32());
    Ok(())
}
