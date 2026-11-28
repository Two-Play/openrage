//! `openrage info`: what an archive or one of its entries is.

use crate::*;

pub(crate) fn cmd_info(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    match vfs::resolve(game_dir, path, keys)? {
        node @ (Node::ArchiveDir(..) | Node::ArchiveFile(..)) => info_rpf7(node),
        node @ (Node::Rpf8Dir(..) | Node::Rpf8File(..)) => info_rpf8(node),
        node @ (Node::Rpf9Dir(..) | Node::Rpf9File(..)) => info_rpf9(node)?,
        node @ (Node::Gta4Dir(..) | Node::Gta4File(..)) => info_gta4(node)?,
        Node::Dir(p) | Node::PlainFile(p) => outln!("{} is not inside an archive", p.display()),
        _ => outln!("{path}: not supported"),
    }
    Ok(())
}

fn info_rpf7(node: Node) {
    match node {
        Node::ArchiveDir(rpf, dir) if dir == Rpf7::ROOT => info_rpf7_root(&rpf),
        Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id) => info_rpf7_entry(&rpf, id),
        _ => {}
    }
}

fn info_rpf8(node: Node) {
    match node {
        Node::Rpf8Dir(rpf, dir) => info_rpf8_dir(&rpf, &dir),
        Node::Rpf8File(rpf, i) => info_rpf8_file(&rpf, i),
        _ => {}
    }
}

fn info_rpf9(node: Node) -> Result<()> {
    match node {
        Node::Rpf9Dir(rpf, dir) => info_rpf9_dir(&rpf, &dir),
        Node::Rpf9File(rpf, i) => info_rpf9_file(&rpf, i),
        _ => Ok(()),
    }
}

fn info_gta4(node: Node) -> Result<()> {
    match node {
        Node::Gta4Dir(a, dir) if dir == rage_rpf::Gta4Archive::ROOT => info_gta4_root(&a),
        Node::Gta4Dir(a, id) | Node::Gta4File(a, id) => {
            info_gta4_entry(&a, id);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn info_rpf7_root(rpf: &Rpf7) {
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

fn info_rpf7_entry(rpf: &Rpf7, id: EntryId) {
    let e = rpf.entry(id);
    outln!("Path:        {}/{}", rpf.display_path(), rpf.path_of(id));
    outln!("Kind:        {}", e.kind_label());
    outln!("Details:     {:?}", e.kind);
    if let Some(v) = e.resource_version() {
        outln!("Resource version: {v}");
    }
}

fn info_rpf8_dir(rpf: &rage_rpf::Rpf8, dir: &str) {
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

fn info_rpf9_dir(rpf: &rage_rpf::Rpf9, dir: &str) -> Result<()> {
    outln!("Archive:     {}", rpf.display_name());
    outln!("Format:      RPF9 (Grand Theft Auto VI)");
    outln!("Size:        {} ({} bytes)", human(rpf.size()), rpf.size());
    outln!("Index slots: {} (declared in header; distinct from recovered streams)", rpf.header().entry_count);
    outln!("TOC tag:     {:#04x} (native transform selector)", rpf.header().toc_transform);
    if rpf.is_native() {
        outln!("Table:       read with the game's executable");
    } else {
        outln!("Table:       encrypted, not read (file names and folders are unknown)");
    }
    outln!("Data:        from offset {:#x}, {}", rpf.header().data_offset, human(rpf.data_len()));
    info_rpf9_coverage(rpf, dir)?;
    if !dir.is_empty() {
        outln!("Folder:      {dir}");
    }
    Ok(())
}

fn info_rpf9_coverage(rpf: &rage_rpf::Rpf9, dir: &str) -> Result<()> {
    // The archive itself is described without reading all of it.
    let known = openrage_app::rpf9::known_entries(rpf);
    let entries = if dir.is_empty() { known } else { Some(rpf.entries()?) };
    match entries {
        Some(entries) => {
            let c = openrage_app::rpf9::coverage(rpf, &entries);
            outln!(
                "{}   {} files, {} in the archive, {} unpacked",
                if rpf.is_native() { "Contents: " } else { "Recovered:" },
                c.files,
                human(c.stored),
                human(c.unpacked)
            );
            if !rpf.is_native() {
                outln!(
                    "Not read:    {} (padding, and data that is neither zlib, Bink 2 nor an audio container)",
                    human(c.unread)
                );
            }
        }
        None => outln!("Recovered:   not indexed yet; `openrage ls` on the archive reads all of it once"),
    }
    Ok(())
}

fn info_rpf9_file(rpf: &rage_rpf::Rpf9, i: usize) -> Result<()> {
    let entries = rpf.entries()?;
    let e = &entries[i];
    outln!("Path:        {}/{}", rpf.display_name(), rpf.entry_name(e));
    if rpf.is_native() {
        outln!("Kind:        {}", if rpf.resource_parts(e).is_some() { "resource" } else { "file" });
    } else {
        outln!("Kind:        recovered file (its name is its offset; the real name is unknown)");
    }
    outln!("Offset:      {:#x}", e.offset);
    outln!("Size:        {} ({} on disk)", human(e.size), human(e.stored));
    match e.kind {
        rage_rpf::rpf9::Kind::Zlib => outln!("Stored as:   {} zlib stream(s)", e.streams),
        rage_rpf::rpf9::Kind::Bink2 => outln!("Stored as:   Bink 2 video"),
        rage_rpf::rpf9::Kind::Awc => outln!("Stored as:   audio container"),
        rage_rpf::rpf9::Kind::Native { slot } => {
            outln!("Slot:        {slot}");
            if let Some((system, graphics)) = rpf.resource_parts(e) {
                outln!("Parts:       {} system, {} graphics", human(system), human(graphics));
            }
        }
    }
    Ok(())
}

fn info_rpf8_file(rpf: &rage_rpf::Rpf8, i: usize) {
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

fn info_gta4_root(a: &rage_rpf::Gta4Archive) -> Result<()> {
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
    Ok(())
}

fn info_gta4_entry(a: &rage_rpf::Gta4Archive, id: rage_rpf::Gta4EntryId) {
    let e = a.entry(id);
    outln!("Path:        {}/{}", a.display_name(), a.entry_path(id));
    outln!("Kind:        {}", openrage_app::gta4::kind_label(a, id));
    match e.kind {
        rage_rpf::Gta4EntryKind::Dir { ref children } => outln!("Entries:     {}", children.len()),
        rage_rpf::Gta4EntryKind::File { stored, size, compressed, .. } => {
            outln!("Size:        {} ({} stored{})", human(size), human(stored), if compressed { ", zlib" } else { "" })
        }
        rage_rpf::Gta4EntryKind::Resource { stored, resource_type, .. } => {
            let (sys, gfx) = e.resource_sizes().unwrap_or_default();
            outln!("Size:        {} (RSC5, zlib)", human(stored));
            outln!("Resource:    type {resource_type} ({})", openrage_app::gta4::resource_type_name(resource_type));
            outln!("Memory:      {} system, {} graphics", human(sys as u64), human(gfx as u64));
        }
    }
}
