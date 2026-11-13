//! File-type icons and human-readable type names.

use slint::Color;

use crate::browser::ItemKind;

const fn rgb(hex: u32) -> Color {
    Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Tint for generic items.
pub const NEUTRAL: Color = rgb(0x7C8592);

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Icon (a name known to the UI's `Icons.get`) and tint for a file, so
/// formats can be told apart at a glance. The tints are mid-tone colors that
/// stay readable on light and dark backgrounds.
pub fn icon(name: &str, kind: ItemKind) -> (&'static str, Color) {
    match kind {
        ItemKind::Dir => return ("folder", rgb(0xC7962C)),
        ItemKind::Archive => return ("archive", rgb(0xD9822B)),
        ItemKind::Texture => return ("texture", rgb(0x9061D8)),
        ItemKind::Sound => return ("audio", rgb(0xDB4F8A)),
        _ => {}
    }
    if let Some(icon) = core_icon(&extension(name)) {
        return icon;
    }
    match crate::plugins::file_type(name) {
        Some((_, category)) => category_icon(&category),
        None => ("file", NEUTRAL),
    }
}

/// Icons of the file types OpenRAGE knows itself.
fn core_icon(ext: &str) -> Option<(&'static str, Color)> {
    Some(match ext {
        "ytd" | "wtd" | "dds" | "png" | "jpg" | "bmp" | "tga" => ("texture", rgb(0x9061D8)),
        "ydr" | "ydd" | "yft" | "yld" | "wdr" | "wdd" | "wft" => ("model", rgb(0x3B82F6)),
        "ypt" => ("particles", rgb(0xE0679B)),
        "ybn" | "wbn" | "wbd" => ("collision", rgb(0x14A08F)),
        "ynv" | "ynd" | "yvr" | "ywr" | "nod" => ("path", rgb(0x14A08F)),
        "ymap" | "wpl" | "ytyp" | "ipl" | "ide" => ("map", rgb(0x2FA866)),
        "ymt" | "ymf" | "meta" | "xml" | "dat" | "json" | "pso" => ("data", rgb(0x2FA866)),
        "cut" => ("anim", rgb(0xDB4F8A)),
        "txt" | "log" | "ini" | "cfg" => ("text", rgb(0x7C8592)),
        "ycd" | "yed" => ("anim", rgb(0xDB4F8A)),
        "awc" | "rel" | "dat151" | "dat54" | "dat4" | "dat10" | "dat15" | "dat16" | "dat22" => ("audio", rgb(0xDB4F8A)),
        "ysc" | "sco" | "asi" | "dll" | "lua" | "cs" => ("script", rgb(0xE05252)),
        "gxt2" | "gxt" => ("lang", rgb(0x5B76DE)),
        "oiv" | "orp" | "zip" | "7z" | "rar" => ("package", rgb(0xD9822B)),
        _ => return None,
    })
}

/// Icon and tint of a plugin's file-type category (`plugin.toml`).
fn category_icon(category: &str) -> (&'static str, Color) {
    match category {
        "texture" => ("texture", rgb(0x9061D8)),
        "model" => ("model", rgb(0x3B82F6)),
        "map" => ("map", rgb(0x2FA866)),
        "data" => ("data", rgb(0x2FA866)),
        "text" => ("text", rgb(0x7C8592)),
        "lang" => ("lang", rgb(0x5B76DE)),
        "audio" => ("audio", rgb(0xDB4F8A)),
        "anim" => ("anim", rgb(0xDB4F8A)),
        "script" => ("script", rgb(0xE05252)),
        "package" => ("package", rgb(0xD9822B)),
        _ => ("file", NEUTRAL),
    }
}

/// Whether OpenRAGE itself knows this file type (plugins only fill the gaps).
pub fn is_known(name: &str) -> bool {
    let ext = extension(name);
    !known_name(&ext).is_empty() || core_icon(&ext).is_some()
}

/// Descriptive type column text.
pub fn type_name(name: &str, kind: ItemKind) -> String {
    let ext = extension(name);
    let known = match kind {
        ItemKind::Dir => "Folder",
        ItemKind::Archive => "Archive",
        ItemKind::Texture => "Texture",
        ItemKind::Sound => "Sound",
        _ => known_name(&ext),
    };
    if !known.is_empty() {
        known.into()
    } else if let Some((plugin_name, _)) = crate::plugins::file_type(name) {
        plugin_name
    } else if ext.is_empty() {
        "File".into()
    } else {
        format!("{} file", ext.to_ascii_uppercase())
    }
}

/// The name of a file type OpenRAGE knows ("" if it does not).
fn known_name(ext: &str) -> &'static str {
    match ext {
        "ytd" | "wtd" => "Texture dictionary",
        "ydr" | "wdr" => "Drawable",
        "ydd" | "wdd" => "Drawable dict",
        "yft" | "wft" => "Fragment",
        "ypt" => "Particles",
        "ybn" | "wbn" => "Bounds",
        "wbd" => "Bounds dict",
        "ynv" => "Nav mesh",
        "ynd" => "Path nodes",
        "ymap" | "wpl" => "Map placement",
        "ytyp" => "Archetypes",
        "ymt" => "Metadata",
        "ymf" => "Manifest",
        "pso" => "Metadata",
        "cut" => "Cutscene",
        "ycd" => "Clip dict",
        "yed" => "Expressions",
        "yld" => "Cloth",
        "ysc" | "sco" => "Script",
        "awc" => "Audio container",
        "gxt2" | "gxt" => "Text table",
        "gfx" => "Scaleform movie",
        "bik" => "Bink video",
        "nametable" => "Name table",
        "ugc" => "UGC (JSON)",
        "orp" => "OpenRAGE plugin",
        "xml" => "XML",
        "meta" => "Meta",
        "dat" => "Data",
        "rel" | "dat151" | "dat54" => "Audio data",
        "txt" => "Text",
        _ => "",
    }
}
