//! A readable structure of metadata XML: groups (structures, array items) as a
//! tree, simple values as named fields, plus helpers for highlighting XML text.

use anyhow::Result;

/// A structure or array item.
#[derive(Debug, Clone, Default)]
pub struct Node {
    /// What the tree shows, e.g. `CEntityDef · dt1_tc_ufocore` or `Entities (12)`.
    pub label: String,
    /// The element name as written in the XML.
    pub tag: String,
    /// Simple values: (readable name, value).
    pub fields: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Node {
    /// Number of nodes in this subtree (including itself).
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }
}

/// Builds the tree of an XML document.
pub fn parse(xml: &str) -> Result<Node> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() },
    )?;
    Ok(node(doc.root_element(), None))
}

/// Fields that name an item, in order of preference.
const NAME_FIELDS: &[&str] =
    &["name", "Name", "archetypeName", "cName", "assetName", "modelName", "Key", "key", "AudioName", "id", "Id"];

fn is_simple(e: roxmltree::Node) -> bool {
    !e.children().any(|c| c.is_element())
}

fn node(e: roxmltree::Node, index: Option<usize>) -> Node {
    let tag = e.tag_name().name().to_owned();
    let mut out = Node { tag: tag.clone(), ..Default::default() };
    let mut items = 0;
    for c in e.children().filter(|c| c.is_element()) {
        if is_simple(c) && c.tag_name().name() != "Item" {
            out.fields.push((humanize(c.tag_name().name()), value(c)));
        } else if is_simple(c) {
            // A simple item of a list (hashes, names, …): keep it as a field.
            items += 1;
            out.fields.push((format!("#{items}"), value(c)));
        } else {
            let i = (c.tag_name().name() == "Item").then(|| {
                items += 1;
                items
            });
            out.children.push(node(c, i));
        }
    }
    // Attributes other than the value ones (e.g. `itemType`) are worth showing too.
    for a in e.attributes() {
        if !matches!(a.name(), "value" | "x" | "y" | "z" | "w" | "type" | "key") {
            out.fields.insert(0, (humanize(a.name()), a.value().to_owned()));
        }
    }
    out.label = label(e, &tag, index, &out);
    out
}

fn label(e: roxmltree::Node, tag: &str, index: Option<usize>, n: &Node) -> String {
    // `rage__cutfCameraObject` is the C++ type `rage::cutfCameraObject`.
    let ty = e.attribute("type").map(|t| t.replace("__", "::"));
    let name = e.attribute("key").map(str::to_owned).or_else(|| {
        NAME_FIELDS
            .iter()
            .find_map(|f| {
                e.children()
                    .find(|c| c.is_element() && c.tag_name().name() == *f && is_simple(*c))
                    .map(value)
                    .filter(|v| !v.is_empty() && v != "—")
            })
            .or_else(|| {
                // Otherwise a field that says it is a name (`handlingName`, `weaponName`, …).
                e.children()
                    .filter(|c| c.is_element() && is_simple(*c))
                    .find(|c| {
                        let t = c.tag_name().name();
                        t.len() > 4 && (t.ends_with("Name") || t.ends_with("_name"))
                    })
                    .map(value)
                    .filter(|v| !v.is_empty() && v != "—")
            })
    });
    let base = match (index, ty) {
        (Some(i), Some(t)) => format!("{t} #{i}"),
        (Some(i), None) => format!("#{i}"),
        (None, Some(t)) => format!("{} ({t})", humanize(tag)),
        // The root element is a type name (`CMapData`), not a field.
        (None, None) if e.parent().is_some_and(|p| p.is_root()) => tag.replace("__", "::"),
        (None, None) => humanize(tag),
    };
    let base = match name {
        Some(name) => format!("{base} · {name}"),
        None => base,
    };
    let items = n.children.len() + n.fields.iter().filter(|(k, _)| k.starts_with('#')).count();
    if index.is_none() && items > 0 && e.children().filter(|c| c.is_element()).all(|c| c.tag_name().name() == "Item") {
        format!("{base} ({items})")
    } else {
        base
    }
}

/// The value of a simple element, formatted for people.
fn value(e: roxmltree::Node) -> String {
    if let Some(v) = e.attribute("value") {
        // Large numbers in fields named like flags read better with their hex form.
        let name = e.tag_name().name().to_ascii_lowercase();
        if name.contains("flag")
            && let Ok(n) = v.parse::<i64>()
            && n > 9
        {
            return format!("{v}  (0x{:X})", n as u32);
        }
        return v.to_owned();
    }
    let axes: Vec<&str> = ["x", "y", "z", "w"].iter().filter_map(|a| e.attribute(*a)).collect();
    if !axes.is_empty() {
        return axes.join(", ");
    }
    let text = e.text().unwrap_or_default().trim();
    if text.is_empty() {
        return "—".into();
    }
    // Raw arrays (numbers on several lines): one line, shortened.
    let words: Vec<&str> = text.split_whitespace().collect();
    if text.contains('\n') && words.len() > 1 {
        let shown: Vec<&str> = words.iter().take(16).copied().collect();
        let more = if words.len() > 16 { format!(" … ({} values)", words.len()) } else { String::new() };
        return format!("{}{more}", shown.join(" "));
    }
    text.to_owned()
}

/// `fTotalDuration` → `Total duration`, `archetypeName` → `Archetype name`,
/// `HDTxdBindingArray` → `HD txd binding array`, `lod_dist` → `Lod dist`.
pub fn humanize(name: &str) -> String {
    if name.starts_with("hash_") || name.is_empty() {
        return name.to_owned();
    }
    let chars: Vec<char> = name.chars().collect();
    // Hungarian prefixes: one lowercase letter before an uppercase one (fValue, iCount, vOffset).
    let start = if chars.len() > 2
        && "fivcbuphsa".contains(chars[0])
        && chars[1].is_ascii_uppercase()
        && chars.get(2).is_some_and(|c| c.is_ascii_lowercase())
    {
        1
    } else {
        0
    };
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for i in start..chars.len() {
        let c = chars[i];
        if c == '_' || c == ' ' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = (i > start).then(|| chars[i - 1]);
        let next = chars.get(i + 1);
        let boundary = c.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase()))
            });
        if boundary && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        // Acronyms (HD, LOD, ID) stay; other words are lowercased after the first.
        let acronym = w.len() > 1 && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if acronym {
            out.push_str(w);
        } else if i == 0 {
            let mut cs = w.chars();
            if let Some(f) = cs.next() {
                out.extend(f.to_uppercase());
                out.push_str(&cs.as_str().to_lowercase());
            }
        } else {
            out.push_str(&w.to_lowercase());
        }
    }
    out
}

/// Kinds of text in a highlighted XML line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// `<`, `>`, `/`, `=`, quotes.
    Punctuation,
    Tag,
    Attribute,
    Value,
    Text,
}

/// Splits one line of XML into highlighted spans (the writer puts at most one
/// element on a line, so a line-local scan is enough).
pub fn highlight(line: &str) -> Vec<(SpanKind, &str)> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    fn push<'a>(out: &mut Vec<(SpanKind, &'a str)>, kind: SpanKind, s: &'a str) {
        if !s.is_empty() {
            out.push((kind, s));
        }
    }
    while i < b.len() {
        if b[i] == b'<' {
            push(&mut out, SpanKind::Text, &line[text_start..i]);
            let end = line[i..].find('>').map_or(b.len(), |e| i + e + 1);
            let tag = &line[i..end];
            // `<`, optional `/` or `?`, name, attributes, `/>` or `>`.
            let mut j = 1;
            while j < tag.len() && matches!(tag.as_bytes()[j], b'/' | b'?' | b'!') {
                j += 1;
            }
            push(&mut out, SpanKind::Punctuation, &tag[..j]);
            let name_end =
                tag[j..].find(|c: char| c.is_whitespace() || c == '>' || c == '/').map_or(tag.len(), |e| j + e);
            push(&mut out, SpanKind::Tag, &tag[j..name_end]);
            let mut k = name_end;
            let tb = tag.as_bytes();
            while k < tag.len() {
                if tb[k] == b'"' {
                    let close = tag[k + 1..].find('"').map_or(tag.len(), |e| k + 1 + e + 1);
                    push(&mut out, SpanKind::Value, &tag[k..close]);
                    k = close;
                } else if tb[k].is_ascii_alphabetic() || tb[k] == b'_' {
                    let e =
                        tag[k..].find(|c: char| c == '=' || c.is_whitespace() || c == '>').map_or(tag.len(), |e| k + e);
                    push(&mut out, SpanKind::Attribute, &tag[k..e]);
                    k = e;
                } else {
                    let e = tag[k..]
                        .find(|c: char| c == '"' || c.is_ascii_alphabetic() || c == '_')
                        .map_or(tag.len(), |e| k + e);
                    push(&mut out, SpanKind::Punctuation, &tag[k..e]);
                    k = e;
                }
            }
            i = end;
            text_start = end;
        } else {
            i += 1;
        }
    }
    push(&mut out, SpanKind::Text, &line[text_start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_names() {
        assert_eq!(humanize("fTotalDuration"), "Total duration");
        assert_eq!(humanize("archetypeName"), "Archetype name");
        assert_eq!(humanize("HDTxdBindingArray"), "HD txd binding array");
        assert_eq!(humanize("lod_dist"), "Lod dist");
        assert_eq!(humanize("bbMin"), "Bb min");
        assert_eq!(humanize("hash_12AB34CD"), "hash_12AB34CD");
        assert_eq!(humanize("position"), "Position");
    }

    #[test]
    fn tree_of_a_map() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<CMapData>
 <name>ufo</name>
 <flags value="1572864" />
 <entities>
  <Item type="CEntityDef">
   <archetypeName>dt1_tc_ufocore</archetypeName>
   <position x="1" y="2" z="3" />
  </Item>
  <Item type="CEntityDef">
   <archetypeName>other</archetypeName>
  </Item>
 </entities>
 <physicsDictionaries>
  <Item>a</Item>
  <Item>b</Item>
 </physicsDictionaries>
 <extents />
</CMapData>"#;
        let root = parse(xml).unwrap();
        assert_eq!(root.label, "CMapData · ufo");
        assert!(root.fields.contains(&("Flags".into(), "1572864  (0x180000)".into())));
        assert!(root.fields.contains(&("Extents".into(), "—".into())));
        let entities = &root.children[0];
        assert_eq!(entities.label, "Entities (2)");
        assert_eq!(entities.children[0].label, "CEntityDef #1 · dt1_tc_ufocore");
        assert!(entities.children[0].fields.contains(&("Position".into(), "1, 2, 3".into())));
        let dicts = &root.children[1];
        assert_eq!(dicts.label, "Physics dictionaries (2)");
        assert_eq!(dicts.fields, vec![("#1".into(), "a".into()), ("#2".into(), "b".into())]);
        assert_eq!(root.count(), 5);
    }

    #[test]
    fn items_named_by_a_name_field() {
        let xml = r#"<CHandlingDataMgr><HandlingData>
  <Item type="CHandlingData"><handlingName>ADDER</handlingName><fMass value="1300" /></Item>
  <Item type="CHandlingData"><fMass value="1" /></Item>
</HandlingData></CHandlingDataMgr>"#;
        let data = &parse(xml).unwrap().children[0];
        assert_eq!(data.children[0].label, "CHandlingData #1 · ADDER");
        assert_eq!(data.children[1].label, "CHandlingData #2");
    }

    #[test]
    fn highlights_a_line() {
        let spans = highlight(r#"  <pos x="1" y="2" />"#);
        let kinds: Vec<SpanKind> = spans.iter().map(|s| s.0).collect();
        assert_eq!(spans[0], (SpanKind::Text, "  "));
        assert!(spans.contains(&(SpanKind::Tag, "pos")));
        assert!(spans.contains(&(SpanKind::Attribute, "x")));
        assert!(spans.contains(&(SpanKind::Value, "\"1\"")));
        assert_eq!(kinds.last(), Some(&SpanKind::Punctuation));
        let joined: String = spans.iter().map(|s| s.1).collect();
        assert_eq!(joined, r#"  <pos x="1" y="2" />"#);
        let t = highlight("<a>text</a>");
        assert!(t.contains(&(SpanKind::Text, "text")));
    }
}
