//! Streaming metadata inventory. Usage: ROOT EXE NEW_OUTPUT_DIRECTORY
use rage_rpf::{rpf9::Archive, rpf9_exe::extract_scarlett_1_0_2_1};
use std::{
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: rpf9_hash_export ROOT EXE NEW_OUTPUT_DIRECTORY".into());
    }
    let root = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[2]);
    let keys = extract_scarlett_1_0_2_1(&PathBuf::from(&args[1]))?;
    fs::create_dir(&output)?;
    let mut entries = BufWriter::new(File::create(output.join("entries.tsv"))?);
    let mut archives = BufWriter::new(File::create(output.join("archives.tsv"))?);
    let mut failures = BufWriter::new(File::create(output.join("errors.tsv"))?);
    let mut names = BufWriter::new(File::create(output.join("name_hashes_all.txt"))?);
    let mut ids = BufWriter::new(File::create(output.join("opaque_ids_all.txt"))?);
    writeln!(
        entries,
        "archive\tindex\tname_hash_64\topaque_id_64\tphysical_offset\tstored_size\tdecoded_size\tresource\ttransformed\tnested_rpf9"
    )?;
    writeln!(archives, "archive\tphysical_base\tarchive_size\tentries\tmetadata_bytes")?;
    writeln!(failures, "archive\terror")?;
    let mut directories = vec![root.clone()];
    let mut roots = Vec::new();
    while let Some(dir) = directories.pop() {
        for item in fs::read_dir(dir)? {
            let item = item?;
            let path = item.path();
            let kind = item.file_type()?;
            if kind.is_dir() && (item.file_name() == "pack" || path.starts_with(root.join("pack"))) {
                directories.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "rpf") {
                roots.push(path);
            }
        }
    }
    roots.sort();
    let root_count = roots.len();
    let mut pending = Vec::new();
    for path in roots.into_iter().rev() {
        let label = path.strip_prefix(&root)?.to_string_lossy().into_owned();
        let size = fs::metadata(&path)?.len();
        pending.push((path, label, 0u64, size, 0usize));
    }
    let (mut archive_count, mut entry_count, mut errors, mut transformed_count, mut detected) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut max_metadata = 0;
    while let Some((path, label, base, size, depth)) = pending.pop() {
        let archive = match Archive::open_range(&path, base, size, &keys) {
            Ok(a) => a,
            Err(e) => {
                errors += 1;
                writeln!(failures, "{label}\t{e}")?;
                continue;
            }
        };
        archive_count += 1;
        max_metadata = max_metadata.max(archive.header.metadata_end);
        writeln!(
            archives,
            "{label}\t{base}\t{size}\t{}\t{}",
            archive.metadata.entries.len(),
            archive.header.metadata_end
        )?;
        let mut source = File::open(&path)?;
        for entry in &archive.metadata.entries {
            entry_count += 1;
            let transformed = entry.crypto.is_some()
                || entry.blocks.is_some()
                || entry.extent.as_ref().is_some_and(|e| e.stream_codec != 0);
            transformed_count += u64::from(transformed);
            let mut nested = false;
            let mut offset = String::new();
            let mut stored = String::new();
            let mut decoded = String::new();
            if let Some(extent) = &entry.extent {
                let physical = base.checked_add(extent.offset).ok_or("physical offset overflow")?;
                offset = physical.to_string();
                stored = extent.stored_size.to_string();
                decoded = extent.unpacked_size.to_string();
                if !transformed && entry.resource_flags.is_none() && extent.stored_size >= 4 {
                    source.seek(SeekFrom::Start(physical))?;
                    let mut magic = [0; 4];
                    source.read_exact(&mut magic)?;
                    nested = magic == *b"9FPR";
                    if nested {
                        detected += 1;
                        let child = format!("{label}!{}", entry.index);
                        if depth >= 16
                            || extent.unpacked_size > extent.stored_size
                            || extent.stored_size - extent.unpacked_size >= 16
                        {
                            errors += 1;
                            writeln!(failures, "{child}\tunsupported nesting depth or stored size")?;
                        } else {
                            pending.push((path.clone(), child, physical, extent.unpacked_size, depth + 1));
                        }
                    }
                }
            }
            let name = entry.name_hash.map(|h| format!("{h:016x}")).unwrap_or_default();
            if !name.is_empty() {
                writeln!(names, "{name}")?;
            }
            writeln!(ids, "{:016x}", entry.hash)?;
            writeln!(
                entries,
                "{label}\t{}\t{name}\t{:016x}\t{offset}\t{stored}\t{decoded}\t{}\t{transformed}\t{nested}",
                entry.index,
                entry.hash,
                entry.resource_flags.is_some()
            )?;
        }
        if archive_count.is_multiple_of(100) {
            eprintln!("{archive_count} archives, {entry_count} entries, {errors} errors");
        }
        // Archive and all its decoded metadata are dropped before the next one.
    }
    entries.flush()?;
    archives.flush()?;
    failures.flush()?;
    names.flush()?;
    ids.flush()?;
    let summary = format!(
        "root_archives={root_count}\nread_archives={archive_count}\nentries={entry_count}\ndetected_nested_archives={detected}\nerrors={errors}\ntransformed_entries_not_probed_for_nested_archives={transformed_count}\nmax_single_metadata_bytes={max_metadata}\n\nCoverage: all entries in readable outer and directly stored nested RPF9 archives.\nTransformed payloads are inventoried but not decoded to discover hidden containers.\nOnly metadata (<=64MiB per archive) and 4-byte direct payload signatures are read.\nHashes are exported as hexadecimal strings. Opaque IDs are not path hashes.\n"
    );
    fs::write(output.join("summary.txt"), &summary)?;
    print!("{summary}");
    Ok(())
}
