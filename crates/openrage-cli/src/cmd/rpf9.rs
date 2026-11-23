//! Experimental Scarlett RPF9 inspection.

use crate::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_rpf9(
    path: &Path,
    exe: Option<&Path>,
    nested: &[u32],
    names: Option<&Path>,
    list: bool,
    extract_raw: Option<u32>,
    extract: Option<u32>,
    output: Option<&Path>,
) -> Result<()> {
    use rage_rpf::rpf9::{Archive, read_header};

    anyhow::ensure!(
        output.is_none() || extract_raw.is_some() || extract.is_some(),
        "--output requires --extract or --extract-raw"
    );
    let (header, file_size) = read_header(path).with_context(|| format!("reading {}", path.display()))?;
    outln!("RPF9 {}", path.display());
    outln!("  size: {file_size:#x}");
    outln!("  archive id: {:#018x}", header.archive_id);
    outln!("  entries: {}", header.entry_count);
    outln!("  metadata: {:#x}..{:#x}", rage_rpf::rpf9::HEADER_SIZE, header.metadata_end);
    outln!(
        "  region B: {:#x}..{:#x} (mode {}, index width {})",
        header.region_b_offset,
        header.metadata_end,
        header.region_b_mode,
        header.index_width
    );
    outln!("  trailer size: {:#x}", header.trailer_size);
    outln!("  profile tag: {:#010x}", header.profile_tag);
    outln!("  crypto profile: {}", header.crypto_profile);

    let Some(exe) = exe else {
        outln!("  metadata: encrypted (pass --exe to decrypt the TOC)");
        return Ok(());
    };
    let extracted = rage_rpf::rpf9_exe::extract_scarlett_1_0_2_1(exe)
        .with_context(|| format!("extracting RPF9 material from {}", exe.display()))?;
    let mut archive = Archive::open(path, &extracted)?;
    for &index in nested {
        archive = archive.open_child(index, &extracted).with_context(|| format!("opening nested entry {index}"))?;
        outln!(
            "  nested entry {index}: archive base {:#x}, {} entries",
            archive.base_offset,
            archive.header.entry_count
        );
    }
    outln!("  TOC: {} entries, key selector {:#x}", archive.metadata.entries.len(), archive.metadata.key_selector);
    let wanted = archive.metadata.entries.iter().filter_map(|e| e.name_hash).collect();
    let names = names.map(|p| rage_rpf::rpf9::NameIndex::load_candidates(p, &wanted)).transpose()?;
    let mut named = 0;
    if list {
        outln!("  index  hash              offset        stored        unpacked      flags   crypto");
        for entry in &archive.metadata.entries {
            match &entry.extent {
                Some(e) => outln!(
                    "  {:5}  {:016x}  {:012x}  {:012x}  {:012x}  {:06x}  {}",
                    entry.index,
                    entry.hash,
                    e.offset,
                    e.stored_size,
                    e.unpacked_size,
                    e.flags,
                    entry.has_crypto_descriptor
                ),
                None => outln!("  {:5}  {:016x}  unsupported storage descriptor", entry.index, entry.hash),
            }
            if let Some(hash) = entry.name_hash
                && let Some(names) = &names
            {
                let candidates: Vec<_> = names.candidates(hash).collect();
                if !candidates.is_empty() {
                    named += 1;
                    outln!("         matched path ({hash:016x}): {}", candidates.join(" | "));
                }
            }
        }
    }
    if let Some(index) = extract_raw.or(extract) {
        let entry = archive.metadata.entries.get(index as usize).context("entry index out of bounds")?;
        anyhow::ensure!(entry.extent.is_some(), "entry has no supported physical extent");
        let output = output.context("export requires --output")?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .with_context(|| format!("creating {}", output.display()))?;
        let result = if extract_raw.is_some() {
            archive.copy_raw(index, &mut file)
        } else {
            archive.copy_decoded(index, &extracted, &mut file)
        };
        let copied = match result {
            Ok(copied) => copied,
            Err(error) => {
                drop(file);
                // This invocation exclusively created the output. Do not leave
                // a partial export looking like a successfully decoded file.
                std::fs::remove_file(output)
                    .with_context(|| format!("removing incomplete {} after {error}", output.display()))?;
                return Err(error.into());
            }
        };
        let kind = if extract_raw.is_some() { "stored" } else { "decoded" };
        outln!("  exported {copied} {kind} bytes to {}", output.display());
    }
    outln!("  {named} entries matched candidate paths; extensions may be absent from the hashed path");
    Ok(())
}
