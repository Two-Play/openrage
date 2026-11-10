//! File-type badges and human-readable type names.

use slint::Color;

use crate::browser::ItemKind;

const fn rgb(hex: u32) -> Color {
    Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Short label and color for the badge shown before a name.
pub fn badge(name: &str, kind: ItemKind) -> (String, Color) {
    match kind {
        ItemKind::Dir => return ("DIR".into(), rgb(0xB8902A)),
        ItemKind::Archive => return ("RPF".into(), rgb(0xD9822B)),
        ItemKind::Texture => return ("TEX".into(), rgb(0x8E5BD6)),
        _ => {}
    }
    let ext = extension(name);
    let color = match ext.as_str() {
        "ytd" | "dds" | "png" => rgb(0x8E5BD6),
        "ydr" | "ydd" | "yft" | "ypt" => rgb(0x3B82F6),
        "ybn" | "ynv" | "ynd" | "yvr" | "ywr" => rgb(0x0F9F8F),
        "ymap" | "ytyp" | "ymt" | "ymf" | "meta" | "xml" | "dat" | "ide" | "ipl" | "txt" | "json" => rgb(0x22A06B),
        "ycd" | "yed" | "yld" | "awc" | "rel" | "dat151" | "dat54" => rgb(0xDB2777),
        "ysc" => rgb(0xDC2626),
        "gxt2" => rgb(0x4F6BD8),
        _ => rgb(0x6B7280),
    };
    let label = if ext.is_empty() { "FILE".into() } else { ext.to_ascii_uppercase().chars().take(4).collect() };
    (label, color)
}

/// Descriptive type column text.
pub fn type_name(name: &str, kind: ItemKind) -> String {
    let ext = extension(name);
    let known = match ext.as_str() {
        _ if kind == ItemKind::Dir => "Folder",
        _ if kind == ItemKind::Archive => "Archive",
        _ if kind == ItemKind::Texture => "Texture",
        "ytd" => "Texture dict",
        "ydr" => "Drawable",
        "ydd" => "Drawable dict",
        "yft" => "Fragment",
        "ypt" => "Particles",
        "ybn" => "Bounds",
        "ynv" => "Nav mesh",
        "ynd" => "Path nodes",
        "ymap" => "Map placement",
        "ytyp" => "Archetypes",
        "ymt" => "Meta (RSC)",
        "ymf" => "Manifest",
        "ycd" => "Clip dict",
        "yed" => "Expressions",
        "yld" => "Cloth",
        "ysc" => "Script",
        "awc" => "Audio",
        "gxt2" => "Text table",
        "xml" => "XML",
        "meta" => "Meta",
        "dat" => "Data",
        "rel" | "dat151" | "dat54" => "Audio data",
        "txt" => "Text",
        _ => "",
    };
    if !known.is_empty() {
        known.into()
    } else if ext.is_empty() {
        "File".into()
    } else {
        format!("{} file", ext.to_ascii_uppercase())
    }
}
