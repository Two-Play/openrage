//! The icon of a game's program (`.exe`), for the game lists. Only the PE
//! headers and the resource section are read; icons stored as PNG are
//! decoded (old-style bitmap icons are skipped).

use std::fs::File;
use std::path::Path;

use rage_core::io::read_exact_at;

/// Resource type of single icon images.
const RT_ICON: u32 = 3;
/// Most bytes of a resource section that are read (icons are near its start
/// in practice; the whole section can be large).
const MAX_RSRC: u32 = 32 << 20;

/// The largest PNG icon of a program as RGBA: (width, height, pixels).
pub fn icon(exe: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = File::open(exe).ok()?;
    let (rsrc, base) = resource_section(&file)?;
    let mut best: Option<(u32, u32, Vec<u8>)> = None;
    for data in icon_images(&rsrc, base) {
        if !data.starts_with(b"\x89PNG\r\n\x1a\n") || data.len() < 24 {
            continue;
        }
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        if best.as_ref().is_some_and(|b| b.0 >= w) || w > 1024 {
            continue;
        }
        if let Ok(decoded) = crate::obj::decode_png(data) {
            best = Some(decoded);
        }
    }
    best
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The `.rsrc` section and its virtual address.
fn resource_section(file: &File) -> Option<(Vec<u8>, u32)> {
    let mut head = vec![0u8; 4096];
    read_exact_at(file, &mut head[..64], 0).ok()?;
    if &head[..2] != b"MZ" {
        return None;
    }
    let pe = u32_at(&head, 0x3C)? as usize;
    let len = file.metadata().ok()?.len();
    let want = (pe + 4096).min(len as usize);
    head.resize(want, 0);
    read_exact_at(file, &mut head, 0).ok()?;
    if head.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(&head, pe + 6)? as usize;
    let optional = u16_at(&head, pe + 20)? as usize;
    let table = pe + 24 + optional;
    for i in 0..sections.min(96) {
        let s = table + i * 40;
        if head.get(s..s + 8)?.starts_with(b".rsrc") {
            let (va, raw_size, raw) = (u32_at(&head, s + 12)?, u32_at(&head, s + 16)?, u32_at(&head, s + 20)?);
            let size = raw_size.min(MAX_RSRC);
            if u64::from(raw) + u64::from(size) > len {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            read_exact_at(file, &mut data, u64::from(raw)).ok()?;
            return Some((data, va));
        }
    }
    None
}

/// The data of every RT_ICON resource: type → name → language → data.
fn icon_images(rsrc: &[u8], base: u32) -> Vec<&[u8]> {
    let entries = |dir: usize| -> Vec<(u32, u32)> {
        let (named, ids) = (u16_at(rsrc, dir + 12).unwrap_or(0), u16_at(rsrc, dir + 14).unwrap_or(0));
        (0..(named as usize + ids as usize).min(4096))
            .filter_map(|k| Some((u32_at(rsrc, dir + 16 + k * 8)?, u32_at(rsrc, dir + 20 + k * 8)?)))
            .collect()
    };
    let subdir = |offset: u32| (offset & 0x8000_0000 != 0).then_some((offset & 0x7FFF_FFFF) as usize);
    let mut out = Vec::new();
    let Some(icons) = entries(0).into_iter().find(|(id, _)| *id == RT_ICON).and_then(|(_, o)| subdir(o)) else {
        return out;
    };
    for (_, name) in entries(icons) {
        let Some(languages) = subdir(name) else { continue };
        for (_, leaf) in entries(languages) {
            if subdir(leaf).is_some() {
                continue;
            }
            let leaf = leaf as usize;
            let (Some(rva), Some(size)) = (u32_at(rsrc, leaf), u32_at(rsrc, leaf + 4)) else { continue };
            let start = rva.checked_sub(base).map(|s| s as usize);
            if let Some(data) = start.and_then(|s| rsrc.get(s..s.checked_add(size as usize)?)) {
                out.push(data);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PE with one RT_ICON resource holding `png`.
    fn tiny_exe(png: &[u8]) -> Vec<u8> {
        let (pe, rsrc_raw, rsrc_va) = (0x40usize, 0x200usize, 0x1000u32);
        let mut rsrc = vec![0u8; 0x60];
        // Root: one id entry (RT_ICON) → subdir at 0x18.
        rsrc[14..16].copy_from_slice(&1u16.to_le_bytes());
        rsrc[16..20].copy_from_slice(&RT_ICON.to_le_bytes());
        rsrc[20..24].copy_from_slice(&(0x8000_0000u32 | 0x18).to_le_bytes());
        // Names: id 1 → subdir at 0x30.
        rsrc[0x18 + 14..0x18 + 16].copy_from_slice(&1u16.to_le_bytes());
        rsrc[0x18 + 16..0x18 + 20].copy_from_slice(&1u32.to_le_bytes());
        rsrc[0x18 + 20..0x18 + 24].copy_from_slice(&(0x8000_0000u32 | 0x30).to_le_bytes());
        // Languages: 1033 → data entry at 0x48.
        rsrc[0x30 + 14..0x30 + 16].copy_from_slice(&1u16.to_le_bytes());
        rsrc[0x30 + 16..0x30 + 20].copy_from_slice(&1033u32.to_le_bytes());
        rsrc[0x30 + 20..0x30 + 24].copy_from_slice(&0x48u32.to_le_bytes());
        // Data entry: RVA, size.
        rsrc[0x48..0x4C].copy_from_slice(&(rsrc_va + 0x60).to_le_bytes());
        rsrc[0x4C..0x50].copy_from_slice(&(png.len() as u32).to_le_bytes());
        rsrc.extend_from_slice(png);
        let mut exe = vec![0u8; rsrc_raw];
        exe[..2].copy_from_slice(b"MZ");
        exe[0x3C..0x40].copy_from_slice(&(pe as u32).to_le_bytes());
        exe[pe..pe + 4].copy_from_slice(b"PE\0\0");
        exe[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
        // No optional header: the section table follows the file header.
        let s = pe + 24;
        exe[s..s + 5].copy_from_slice(b".rsrc");
        exe[s + 12..s + 16].copy_from_slice(&rsrc_va.to_le_bytes());
        exe[s + 16..s + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        exe[s + 20..s + 24].copy_from_slice(&(rsrc_raw as u32).to_le_bytes());
        exe.extend(rsrc);
        exe
    }

    #[test]
    fn reads_the_png_icon_of_a_program() {
        let png = rage_formats::png(2, 2, &[255; 16]).unwrap();
        let dir = std::env::temp_dir().join(format!("openrage-exe-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Game.exe");
        std::fs::write(&exe, tiny_exe(&png)).unwrap();
        assert_eq!(icon(&exe).map(|(w, h, p)| (w, h, p.len())), Some((2, 2, 16)));
        // Damaged files give no icon, not a panic.
        let mut bad = tiny_exe(&png);
        bad.truncate(0x220);
        std::fs::write(&exe, bad).unwrap();
        assert!(icon(&exe).is_none());
        std::fs::write(&exe, b"MZ").unwrap();
        assert!(icon(&exe).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
