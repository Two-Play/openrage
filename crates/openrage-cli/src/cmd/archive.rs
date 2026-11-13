//! Browsing and extracting archives, keys and licences.

use crate::*;

pub(crate) fn cmd_gta4_keys(game_dir: &Path) -> Result<()> {
    let (_, report) = openrage_app::gta4::derive_key(game_dir)?;
    outln!("GTA IV archive key found in {} ({:.1}s).", report.exe.display(), report.scan_seconds);
    if let Some(n) = report.verified_entries {
        outln!("Verified: vehicles.img decrypts ({n} entries).");
    }
    outln!("Key cached at {} (local only, never distribute this file).", report.cache_path.display());
    Ok(())
}

pub(crate) fn print_gta4_entry(archive: &rage_rpf::Gta4Archive, id: rage_rpf::Gta4EntryId, path: &str) {
    let e = archive.entry(id);
    let kind = openrage_app::gta4::kind_label(archive, id);
    let ty = e.resource_type().map(|t| format!("t{t}")).unwrap_or_default();
    let slash = if e.is_dir() { "/" } else { "" };
    outln!("{kind:<8} {:>8} {ty:>4}  {path}{slash}", human(e.size()));
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

pub(crate) fn print_entry(rpf: &Rpf7, id: EntryId, path: &str) {
    let e = rpf.entry(id);
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    let suffix = if e.is_dir() || e.is_archive() { "/" } else { "" };
    outln!("{:<8} {:>8} {:>4}  {path}{suffix}", e.kind_label(), human(e.size()), version);
}

pub(crate) fn cmd_ls(game_dir: &Path, path: &str, recursive: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::Dir(dir) => {
            let mut items: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
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
                let slash = if kind == "file" { "" } else { "/" };
                outln!("{kind:<8} {:>8}       {name}{slash}", human(meta.len()));
            }
        }
        Node::PlainFile(p) => outln!("file     {:>8}       {}", human(fs::metadata(&p)?.len()), p.display()),
        Node::ArchiveFile(rpf, id) => print_entry(&rpf, id, &rpf.path_of(id)),
        Node::Rpf8File(rpf, i) => print_rpf8_entry(&rpf, i, &rpf.entry_name(i)),
        Node::Rpf8Dir(rpf, dir) => {
            let (dirs, files) = rpf.list_dir(&dir);
            for d in dirs {
                outln!("{:<8} {:>8}       {d}/", "dir", "");
            }
            for i in files {
                let name = rpf.entry_name(i);
                print_rpf8_entry(&rpf, i, name.rsplit('/').next().unwrap_or(&name));
            }
        }
        Node::Gta4File(a, id) => print_gta4_entry(&a, id, &a.entry_path(id)),
        Node::Gta4Dir(a, dir) => {
            if recursive {
                vfs::walk_gta4(&a, dir, "", &mut |id, p| print_gta4_entry(&a, id, p));
            } else {
                for id in a.children(dir) {
                    print_gta4_entry(&a, id, &a.entry(id).name);
                }
            }
        }
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

pub(crate) fn cmd_info(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
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
        Node::Rpf8Dir(rpf, dir) => {
            let h = rpf.header();
            outln!("Archive:     {}", rpf.display_name());
            outln!("Format:      RPF8 (Red Dead Redemption 2)");
            outln!("Size:        {} ({} bytes)", human(rpf.size()), rpf.size());
            outln!(
                "Table:       {}",
                if h.is_encrypted() { format!("encrypted (key {})", h.decryption_tag) } else { "plaintext".into() }
            );
            outln!("Entries:     {}", h.entry_count);
            if !dir.is_empty() {
                outln!("Folder:      {dir}");
            }
        }
        Node::Rpf8File(rpf, i) => {
            let e = &rpf.entries().expect("entries")[i];
            outln!("Path:        {}/{}", rpf.display_name(), rpf.entry_name(i));
            outln!("Kind:        {}", if e.is_resource { "resource" } else { "file" });
            outln!("Size:        {} ({} on disk)", human(e.size()), human(e.size_on_disk));
            outln!("Compression: {:?}", e.compression);
            outln!("Encrypted:   {}", if e.is_encrypted() { format!("yes (key {})", e.key_id) } else { "no".into() });
            if let Some(v) = e.resource_version() {
                outln!("Resource version: {v}");
                outln!(
                    "Pages:       {} virtual, {} physical",
                    e.size_or_flags & 0xFFFF_FFF0,
                    e.size_or_flags >> 32 & 0xFFFF_FFF0
                );
            }
        }
        Node::Gta4Dir(a, dir) if dir == rage_rpf::Gta4Archive::ROOT => {
            let size = fs::metadata(a.path())?.len();
            let ids = 1..a.len();
            let dirs = ids.clone().filter(|&i| a.entry(i).is_dir()).count();
            let resources = ids.clone().filter(|&i| a.entry(i).resource_type().is_some()).count();
            outln!("Archive:     {}", a.display_name());
            outln!("Format:      {} (GTA IV)", a.format().name());
            outln!("Size:        {} ({size} bytes)", human(size));
            outln!("Table:       {}", if a.is_encrypted() { "encrypted (AES)" } else { "plaintext" });
            outln!(
                "Entries:     {} ({dirs} dirs, {} files, {resources} resources)",
                a.len() - 1,
                a.len() - 1 - dirs - resources
            );
        }
        Node::Gta4Dir(a, id) | Node::Gta4File(a, id) => {
            let e = a.entry(id);
            outln!("Path:        {}/{}", a.display_name(), a.entry_path(id));
            outln!("Kind:        {}", openrage_app::gta4::kind_label(&a, id));
            match e.kind {
                rage_rpf::Gta4EntryKind::Dir { ref children } => outln!("Entries:     {}", children.len()),
                rage_rpf::Gta4EntryKind::File { stored, size, compressed, .. } => {
                    outln!(
                        "Size:        {} ({} stored{})",
                        human(size),
                        human(stored),
                        if compressed { ", zlib" } else { "" }
                    )
                }
                rage_rpf::Gta4EntryKind::Resource { stored, resource_type, .. } => {
                    let (sys, gfx) = e.resource_sizes().unwrap_or_default();
                    outln!("Size:        {} (RSC5, zlib)", human(stored));
                    outln!(
                        "Resource:    type {resource_type} ({})",
                        openrage_app::gta4::resource_type_name(resource_type)
                    );
                    outln!("Memory:      {} system, {} graphics", human(sys as u64), human(gfx as u64));
                }
            }
        }
        Node::Dir(p) | Node::PlainFile(p) => outln!("{} is not inside an archive", p.display()),
    }
    Ok(())
}

pub(crate) fn report_rpf8_extract(rpf: &rage_rpf::Rpf8, dir: &str, file: Option<usize>, out: &Path) -> Result<()> {
    let s = openrage_app::extract::extract_rpf8(rpf, dir, file, out, &AtomicBool::new(false), &mut |_, _| {})?;
    for f in &s.failures {
        eprintln!("warning: {f}");
    }
    outln!("{} file(s) ({}) extracted to {}, {} failed", s.files, human(s.bytes), out.display(), s.failures.len());
    extraction_result(s.failures.len())
}

/// Extraction goes on past single failures, but the command still fails then,
/// so scripts notice.
pub(crate) fn extraction_result(failures: usize) -> Result<()> {
    if failures > 0 {
        bail!("{failures} file(s) could not be extracted (see the warnings above)");
    }
    Ok(())
}

pub(crate) fn print_rpf8_entry(rpf: &rage_rpf::Rpf8, i: usize, name: &str) {
    let e = &rpf.entries().expect("entries")[i];
    let (kind, slash) = if e.ext_id == 0 {
        ("rpf", "/")
    } else if e.is_resource {
        ("resource", "")
    } else {
        ("file", "")
    };
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    outln!("{kind:<8} {:>8} {version:>4}  {name}{slash}", human(e.size()));
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
