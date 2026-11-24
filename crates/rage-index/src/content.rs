//! Turns indexable files into searchable text.

use rage_core::hash::{jenkins_bytes, joaat};
use rage_core::io::u32_le;
use rage_core::text::{decode_text, identifiers};
use rage_rpf::vfs::Node;
use rage_rpf::{Rpf9, RpfError, rpf9};

const MAX_TEXT: usize = 4 << 20;

/// Extensions of plain-text formats worth indexing.
const TEXT_EXTENSIONS: &[&str] =
    &["xml", "meta", "dat", "txt", "ide", "ipl", "ugc", "json", "ini", "cfg", "csv", "lua", "log"];

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Whether a binary file with this name contributes text to the index.
pub fn is_indexable(name: &str) -> bool {
    let ext = extension(name);
    ext == "gxt2" || ext == "gxt" || ext == "nametable" || TEXT_EXTENSIONS.contains(&ext.as_str())
}

/// Extensions of GTA VI files that hold metadata, beside the above. Maps and
/// archetypes (`#map`, `#typ`) are left out, as in the other games.
const GTA6_EXTENSIONS: &[&str] = &["#mt", "#ldbmt", "pso", "psin", "rbf0", "cut"];

/// Like [`is_indexable`], for a file of a GTA VI archive.
pub fn is_indexable_gta6(name: &str) -> bool {
    is_indexable(name) || GTA6_EXTENSIONS.contains(&extension(name).as_str())
}

/// Searchable text for a file of a GTA VI archive: text as in other games,
/// metadata as the XML it is shown as. The names in that are hashes
/// (`hash_1234ABCD`), which searches for a name or hash look for as well.
pub fn rpf9_content(rpf: &Rpf9, entry: &rpf9::Entry, name: &str) -> Result<Option<(String, Vec<String>)>, RpfError> {
    let data = rpf.read(entry)?;
    let parts = rpf.resource_parts(entry);
    Ok(gta6_content(name, data, parts.map(|(system, _)| system as usize)))
}

fn gta6_content(name: &str, mut data: Vec<u8>, system: Option<usize>) -> Option<(String, Vec<String>)> {
    use rage_formats::{meta, psin, pso, rbf};
    if let Some(system) = system {
        if !meta::is_gta6(&data) || system > data.len() {
            return None;
        }
        let graphics = data.split_off(system);
        let resource = rage_resource::Resource { version: meta::VERSION_RDR2, system: data, graphics };
        let meta = meta::parse(&resource).ok();
        if extension(name) == "#ldbmt" {
            // Like the string tables of other games.
            let texts = meta?.label_texts();
            return Some((texts.iter().map(|(label, text)| format!("0x{label:08X} {text}\n")).collect(), Vec::new()));
        }
        return metadata_content(meta.map(|m| m.to_xml()), &resource.system);
    }
    if psin::is_psin(&data) {
        return metadata_content(psin::parse(&data).ok().map(|p| p.to_xml()), &data);
    }
    if pso::is_pso(&data) {
        return metadata_content(pso::parse(&data).ok().map(|p| p.to_xml()), &data);
    }
    if rbf::is_rbf(&data) {
        return metadata_content(rbf::to_xml(&data).ok().map(|(xml, errors, _)| (xml, errors)), &data);
    }
    // Metadata that is text already.
    content_for(if is_indexable(name) { name } else { "metadata.xml" }, &data)
}

/// The words of metadata without its numbers, which make up most of it and
/// are no use in a search. Words stay in order, for phrases.
pub fn without_numbers(text: &str) -> String {
    let is_number =
        |w: &str| w.as_bytes()[0].is_ascii_digit() && (w.len() < 3 || w.bytes().all(|b| b.is_ascii_digit()));
    let words = text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty() && !is_number(w));
    let mut out = String::new();
    for word in words {
        out.push_str(word);
        out.push(' ');
    }
    out
}

/// Metadata as XML; with the strings in the data where that could not be
/// converted in full. Hashes that stand for unknown names are no names.
fn metadata_content(converted: Option<(String, Vec<String>)>, data: &[u8]) -> Option<(String, Vec<String>)> {
    let (mut text, incomplete) = match converted {
        Some((xml, errors)) => (xml, !errors.is_empty()),
        None => (String::new(), true),
    };
    if incomplete {
        let printable = |b: &u8| *b >= b' ' && *b != 0x7f;
        for run in data.split(|b| !printable(b)).filter(|run| run.len() >= 4) {
            if let Ok(run) = std::str::from_utf8(run) {
                text.push('\n');
                text.push_str(run);
            }
        }
    }
    let mut words: Vec<String> =
        identifiers(&text).filter(|word| !word.starts_with("hash_")).map(str::to_owned).collect();
    words.sort_unstable();
    words.dedup();
    (!text.is_empty()).then_some((text, words))
}

/// The text of an indexed file, read back from where it is.
pub fn node_text(node: &Node) -> Option<String> {
    let of = |name: &str, data: &[u8]| content_for(name, data).map(|(text, _)| text);
    match node {
        Node::ArchiveFile(rpf, id) => of(&rpf.entry(*id).name, &rpf.read(*id).ok()?),
        Node::Gta4File(archive, id) => of(&archive.entry(*id).name, &archive.read(*id).ok()?),
        Node::PlainFile(path) => of(&path.file_name()?.to_string_lossy(), &std::fs::read(path).ok()?),
        Node::Rpf8File(rpf, i) => of(&rpf.entry_name(*i), &rpf.read(rpf.entries()?.get(*i)?).ok()?),
        Node::Rpf9File(rpf, i) => {
            let entries = rpf.entries().ok()?;
            let entry = entries.get(*i)?;
            let name = rpf.entry_name(entry);
            if !is_indexable_gta6(&name) {
                return None;
            }
            rpf9_content(rpf, entry, &name).ok()?.map(|(text, _)| text)
        }
        _ => None,
    }
}

/// How metadata of unknown names refers to `name`: by its hash, of the name
/// as it is (fields and structures) or in lower case (values).
pub(crate) fn hashed_names(name: &str) -> Option<[String; 2]> {
    let one_name = name.len() >= 3 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    one_name.then(|| [jenkins_bytes(name.as_bytes()), joaat(name)].map(|hash| format!("hash_{hash:08x}")))
}

/// Searchable text for a file, plus names for the hash dictionary.
///
/// * text formats: the text itself; dictionary = identifiers in it
/// * `.nametable`: one name per line; dictionary = all names
/// * `.gxt2`, GTA IV `.gxt`: one `0xLABELHASH text` line per string; no
///   dictionary entries
pub fn content_for(name: &str, data: &[u8]) -> Option<(String, Vec<String>)> {
    match extension(name).as_str() {
        "gxt2" => parse_gxt2(data).map(|body| (body, Vec::new())),
        "gxt" => parse_gxt(data).map(|body| (body, Vec::new())),
        "nametable" => {
            let names: Vec<String> = data
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            (!names.is_empty()).then(|| (names.join("\n"), names))
        }
        ext if TEXT_EXTENSIONS.contains(&ext) => {
            let text = decode_text(data, MAX_TEXT)?;
            let mut dict: Vec<String> = identifiers(&text).map(str::to_owned).collect();
            dict.sort_unstable();
            dict.dedup();
            Some((text, dict))
        }
        _ => None,
    }
}

/// GXT2: `"2TXG"`, count, `count × (label hash, absolute offset)`, then NUL-terminated UTF-8 strings.
fn parse_gxt2(data: &[u8]) -> Option<String> {
    if data.len() < 8 || &data[0..4] != b"2TXG" {
        return None;
    }
    let count = u32_le(data, 4) as usize;
    let table_end = 8usize.checked_add(count.checked_mul(8)?)?;
    if table_end > data.len() {
        return None;
    }
    let mut out = String::new();
    for i in 0..count {
        let hash = u32_le(data, 8 + i * 8);
        let offset = u32_le(data, 12 + i * 8) as usize;
        let Some(tail) = data.get(offset..) else { continue };
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        out.push_str(&format!("0x{hash:08X} {}\n", String::from_utf8_lossy(&tail[..end])));
    }
    Some(out)
}

/// GTA IV GXT: `u16` 4, `u16` 16, `TABL` (12-byte records: name, offset);
/// each table (after its 8-byte name, except `MAIN`) has a `TKEY` block of
/// (offset, hash) pairs and a `TDAT` block of NUL-terminated UTF-16 strings.
fn parse_gxt(data: &[u8]) -> Option<String> {
    if data.len() < 12 || data[0..4] != [4, 0, 16, 0] || &data[4..8] != b"TABL" {
        return None;
    }
    let block = |at: usize, magic: &[u8; 4]| -> Option<(usize, usize)> {
        (data.get(at..at + 4)? == magic).then(|| (at + 8, u32_le(data, at + 4) as usize))
    };
    let (tabl, tabl_len) = block(4, b"TABL")?;
    let mut out = String::new();
    for k in 0..tabl_len / 12 {
        let rec = tabl + k * 12;
        let name = data.get(rec..rec + 8)?;
        let mut at = u32_le(data, rec + 8) as usize;
        if !name.starts_with(b"MAIN\0") {
            at += 8;
        }
        let (keys, keys_len) = block(at, b"TKEY")?;
        let (strings, _) = block(keys + keys_len, b"TDAT")?;
        for i in 0..keys_len / 8 {
            let offset = u32_le(data, keys + i * 8) as usize;
            let hash = u32_le(data, keys + i * 8 + 4);
            let start = strings + offset;
            let units: Vec<u16> = data
                .get(start..)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .collect();
            out.push_str(&format!("0x{hash:08X} {}\n", String::from_utf16_lossy(&units)));
        }
    }
    Some(out)
}

/// The line of `text` with the most of the query terms (the first such,
/// case-insensitive), shortened.
/// Names and hashes (`0x1234ABCD`) are also found as `hash_1234ABCD`.
pub fn snippet(text: &str, query: &str) -> Option<String> {
    let query = query.trim();
    let terms: Vec<String> = query.split_whitespace().map(|t| t.trim_matches('"').to_lowercase()).collect();
    let find = |terms: &[String]| {
        let mut best = (0, None);
        for line in text.lines() {
            let lower = line.to_lowercase();
            let found = terms.iter().filter(|t| !t.is_empty() && lower.contains(t.as_str())).count();
            if found > best.0 {
                best = (found, Some(line));
            }
            if found == terms.len() {
                break;
            }
        }
        best.1
    };
    let hashed = || match query.strip_prefix("0x") {
        Some(hash) if hash.len() == 8 => find(&[format!("hash_{}", hash.to_lowercase())]),
        _ => find(&hashed_names(query)?),
    };
    let line = find(&terms).or_else(hashed)?;
    let line = line.trim();
    Some(if line.chars().count() > 200 {
        format!("{}…", line.chars().take(200).collect::<String>())
    } else {
        line.into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gxt2(entries: &[(u32, &str)]) -> Vec<u8> {
        let mut out = b"2TXG".to_vec();
        out.extend((entries.len() as u32).to_le_bytes());
        let mut strings = Vec::new();
        let base = 8 + entries.len() * 8 + 8;
        for (hash, text) in entries {
            out.extend(hash.to_le_bytes());
            out.extend(((base + strings.len()) as u32).to_le_bytes());
            strings.extend(text.as_bytes());
            strings.push(0);
        }
        out.extend(b"2TXG");
        out.extend(((base + strings.len()) as u32).to_le_bytes());
        out.extend(strings);
        out
    }

    #[test]
    fn parses_gta4_gxt() {
        // One MAIN table with one string.
        let mut d = vec![4, 0, 16, 0];
        d.extend(b"TABL");
        d.extend(12u32.to_le_bytes());
        d.extend(b"MAIN\0\0\0\0");
        d.extend(24u32.to_le_bytes());
        d.extend(b"TKEY");
        d.extend(8u32.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(0x22197u32.to_le_bytes());
        d.extend(b"TDAT");
        d.extend(10u32.to_le_bytes());
        for u in "HOME".encode_utf16().chain([0]) {
            d.extend(u.to_le_bytes());
        }
        let (body, dict) = content_for("american.gxt", &d).unwrap();
        assert_eq!(body, "0x00022197 HOME\n");
        assert!(dict.is_empty());
    }

    #[test]
    fn parses_gxt2() {
        let (body, dict) = content_for("global.gxt2", &gxt2(&[(0xB779_A091, "Adder"), (1, "Hello")])).unwrap();
        assert_eq!(body, "0xB779A091 Adder\n0x00000001 Hello\n");
        assert!(dict.is_empty());
    }

    #[test]
    fn nametable_and_text() {
        let (body, dict) = content_for("a.nametable", b"one\0two\0").unwrap();
        assert_eq!(body, "one\ntwo");
        assert_eq!(dict, ["one", "two"]);
        let (_, dict) = content_for("x.meta", b"<Item>WEAPON_PISTOL</Item>").unwrap();
        assert_eq!(dict, ["Item", "WEAPON_PISTOL"]);
        assert!(content_for("x.ydr", b"abc").is_none());
    }

    #[test]
    fn metadata_is_searched_without_numbers() {
        let xml = "<a x=\"1.5e-05\">-12 3d_prop 0x1F4 pack.name</a>";
        assert_eq!(without_numbers(xml), "a x 3d_prop 0x1F4 pack name a ");
    }

    #[test]
    fn metadata_words_leave_out_unknown_hashes() {
        let xml = "<hash_1234ABCD>weapon_pistol</hash_1234ABCD>".to_owned();
        let (text, words) = metadata_content(Some((xml.clone(), Vec::new())), b"data").unwrap();
        assert_eq!((text, words), (xml, vec!["weapon_pistol".to_owned()]));
    }

    #[test]
    fn strings_stand_in_for_metadata_that_cannot_be_converted() {
        let data = b"\x01\x02body_shell\0ab\0\xff\xfebinary\0Geb\xc3\xa4ude\0";
        let (text, words) = metadata_content(None, data).unwrap();
        assert_eq!(text, "\nbody_shell\nGebäude");
        assert_eq!(words, ["Geb", "body_shell", "ude"]);
        assert!(metadata_content(None, b"\0\x01").is_none());
        let (text, _) = metadata_content(Some(("<a/>".to_owned(), vec!["an error".to_owned()])), data).unwrap();
        assert!(text.starts_with("<a/>\nbody_shell"));
    }

    #[test]
    fn finds_hashed_names() {
        let [exact, lower] = hashed_names("Adder").unwrap();
        assert_eq!(lower, "hash_b779a091");
        assert_ne!(exact, lower);
        assert!(hashed_names("two words").is_none() && hashed_names("a.b").is_none());
        let text = "<a>\n <hash_B779A091/>\n</a>";
        for query in ["Adder", "0xB779A091"] {
            assert_eq!(snippet(text, query).as_deref(), Some("<hash_B779A091/>"));
        }
        assert_eq!(snippet(text, "0x"), None);
    }

    #[test]
    fn finds_snippet() {
        assert_eq!(snippet("a gun\nthe gun of A", "of gun").as_deref(), Some("the gun of A"));
        assert_eq!(snippet("a\n  <b>Pistol</b>\nc", "pistol").as_deref(), Some("<b>Pistol</b>"));
    }
}
