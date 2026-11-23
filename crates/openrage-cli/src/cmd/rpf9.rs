//! Experimental Scarlett RPF9 inspection.

use crate::*;

pub(crate) fn cmd_rpf9(path: &Path, exe: Option<&Path>) -> Result<()> {
    use rage_rpf::rpf9::{decrypt_first_metadata_span, read_header};

    let (header, file_size) =
        read_header(path).with_context(|| format!("reading {}", path.display()))?;
    outln!("RPF9 {}", path.display());
    outln!("  size: {file_size:#x}");
    outln!("  archive id: {:#018x}", header.archive_id);
    outln!("  entries: {}", header.entry_count);
    outln!(
        "  metadata: {:#x}..{:#x}",
        rage_rpf::rpf9::HEADER_SIZE,
        header.metadata_end
    );
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
        outln!("  metadata: encrypted (pass --exe to validate the first TFIT-CBC span)");
        return Ok(());
    };
    let extracted = rage_rpf::rpf9_exe::extract_scarlett_1_0_2_1(exe)
        .with_context(|| format!("extracting RPF9 material from {}", exe.display()))?;
    let key_va = extracted.descriptor.data_va;
    let plain = decrypt_first_metadata_span(path, &extracted.material.decryptor())?;
    outln!("  TFIT material: verified (key descriptor at VA {key_va:#x}; no key bytes printed)");
    outln!("  first metadata span ({} bytes):", plain.len());
    for (line, chunk) in plain.chunks(16).enumerate() {
        let hex = chunk
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        outln!("    {:04x}: {hex}", line * 16);
    }
    outln!("  full TOC mapping is not implemented yet; extraction and writing stay disabled");
    Ok(())
}
