//! XML edits for packages: an XPath subset to select nodes, and edits applied
//! to the original text so indentation, comments and formatting stay intact.
//!
//! Supported XPath: absolute or relative paths, `//` descendant steps, `*`,
//! `.`, `..`, and predicates `[N]`, `[last()]`, `[. = 'v']`, `[text() = 'v']`,
//! `[@attr = 'v']`, `[child = 'v']` (also `!=`), `[@attr]`, `[child]`,
//! `[contains(x, 'v')]` and `[starts-with(x, 'v')]`.

use anyhow::{Context, Result, bail};
use roxmltree::{Document, Node};

use super::package::{Append, XmlCmd};

// ---------------------------------------------------------------------------
// XPath subset

/// Splits on `sep` outside brackets, parentheses and quotes.
fn split_outside(s: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut quote, mut start) = (0i32, None, 0);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '[' | '(') => depth += 1,
            (None, ']' | ')') => depth -= 1,
            (None, c) if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn unquote(s: &str) -> Option<&str> {
    let s = s.trim();
    let q = s.chars().next()?;
    (matches!(q, '\'' | '"') && s.len() >= 2 && s.ends_with(q)).then(|| &s[1..s.len() - 1])
}

fn string_value(n: Node<'_, '_>) -> String {
    n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect()
}

/// Value of a predicate operand for node `n`: `.`, `text()`, `@attr`, or a child element name.
fn operand(n: Node<'_, '_>, expr: &str) -> Option<String> {
    let e = expr.trim();
    if e == "." {
        return Some(string_value(n));
    }
    if e == "text()" {
        return Some(n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect());
    }
    if let Some(a) = e.strip_prefix('@') {
        return n.attribute(a).map(str::to_owned);
    }
    n.children().find(|c| c.is_element() && c.tag_name().name() == e).map(string_value)
}

fn values_equal(a: &str, b: &str) -> bool {
    a == b || a.trim() == b.trim()
}

/// Filters `nodes` (the matches of one step for one context node) by a predicate.
fn apply_predicate<'a>(nodes: Vec<Node<'a, 'a>>, pred: &str) -> Result<Vec<Node<'a, 'a>>> {
    let p = pred.trim();
    if let Ok(n) = p.parse::<usize>() {
        return Ok(nodes.get(n.wrapping_sub(1)).copied().into_iter().collect());
    }
    if p == "last()" {
        return Ok(nodes.last().copied().into_iter().collect());
    }
    for (func, test) in [("contains(", 0), ("starts-with(", 1)] {
        if let Some(args) = p.strip_prefix(func).and_then(|r| r.strip_suffix(')')) {
            let parts = split_outside(args, ',');
            let [x, v] = parts.as_slice() else { bail!("cannot parse predicate [{p}]") };
            let v = unquote(v).with_context(|| format!("expected a quoted string in [{p}]"))?;
            return Ok(nodes
                .into_iter()
                .filter(|n| operand(*n, x).is_some_and(|s| if test == 0 { s.contains(v) } else { s.starts_with(v) }))
                .collect());
        }
    }
    let (left, right, negate) = if let Some(i) = p.find("!=") {
        (&p[..i], Some(&p[i + 2..]), true)
    } else if let Some(i) = p.find('=') {
        (&p[..i], Some(&p[i + 1..]), false)
    } else {
        (p, None, false)
    };
    Ok(match right {
        None => nodes.into_iter().filter(|n| operand(*n, left).is_some()).collect(),
        Some(r) => {
            let r = r.trim();
            let value = unquote(r).map(str::to_owned).unwrap_or_else(|| r.to_owned());
            nodes
                .into_iter()
                .filter(|n| operand(*n, left).is_some_and(|v| values_equal(&v, &value) != negate))
                .collect()
        }
    })
}

/// Index of the `]` closing a predicate whose `[` was just consumed.
fn closing_bracket(s: &str) -> Option<usize> {
    let (mut depth, mut quote) = (0i32, None);
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '[') => depth += 1,
            (None, ']') if depth == 0 => return Some(i),
            (None, ']') => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Selects element nodes with an XPath subset.
pub fn select<'a>(doc: &'a Document<'a>, xpath: &str) -> Result<Vec<Node<'a, 'a>>> {
    let xpath = xpath.trim();
    let mut current = vec![if xpath.starts_with('/') { doc.root() } else { doc.root_element() }];
    let steps = split_outside(xpath.trim_start_matches('/'), '/');
    let mut descendant = xpath.starts_with("//");
    for step in steps {
        if step.is_empty() {
            descendant = true; // `//`
            continue;
        }
        let (name, preds) = match step.find('[') {
            Some(i) => (&step[..i], &step[i..]),
            None => (step, ""),
        };
        let mut predicates = Vec::new();
        let mut rest = preds;
        while let Some(stripped) = rest.strip_prefix('[') {
            let close = closing_bracket(stripped).context("unbalanced [ ] in XPath")?;
            predicates.push(&stripped[..close]);
            rest = &stripped[close + 1..];
        }
        let mut next: Vec<Node<'a, 'a>> = Vec::new();
        for ctx in &current {
            let mut matches: Vec<Node<'a, 'a>> = match name {
                "." => vec![*ctx],
                ".." => ctx.parent().into_iter().filter(|p| p.is_element() || p.is_root()).collect(),
                _ => {
                    let pool: Vec<Node<'a, 'a>> = if descendant {
                        ctx.descendants().filter(|d| d.id() != ctx.id()).collect()
                    } else {
                        ctx.children().collect()
                    };
                    pool.into_iter()
                        .filter(|n| n.is_element() && (name == "*" || n.tag_name().name() == name))
                        .collect()
                }
            };
            for p in &predicates {
                matches = apply_predicate(matches, p)?;
            }
            for m in matches {
                if !next.iter().any(|x| x.id() == m.id()) {
                    next.push(m);
                }
            }
        }
        current = next;
        descendant = false;
    }
    Ok(current.into_iter().filter(|n| n.is_element()).collect())
}

// ---------------------------------------------------------------------------
// Text-preserving edits

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Whitespace before `pos` on its line, if the line has nothing else before it.
fn leading_indent(text: &str, pos: usize) -> Option<&str> {
    let ls = line_start(text, pos);
    let before = &text[ls..pos];
    before.chars().all(|c| c == ' ' || c == '\t').then_some(before)
}

/// End of the start tag of an element (index just past `>`), and whether it is self-closing.
fn start_tag_end(text: &str, node: Node<'_, '_>) -> (usize, bool) {
    let start = node.range().start;
    let mut quote = None;
    for (i, c) in text[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return (start + i + 1, text[..start + i].ends_with('/')),
            _ => {}
        }
    }
    (node.range().end, true)
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn indent_block(content: &str, indent: &str, nl: &str) -> String {
    content.lines().map(|l| format!("{indent}{l}")).collect::<Vec<_>>().join(nl)
}

/// Indentation for children of `node`.
fn child_indent(text: &str, node: Node<'_, '_>) -> String {
    let own = leading_indent(text, node.range().start).unwrap_or("").to_owned();
    // The most common indentation among the children (files are not always consistent).
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for c in node.children().filter(|c| c.is_element()) {
        if let Some(ci) = leading_indent(text, c.range().start) {
            match counts.iter_mut().find(|(i, _)| *i == ci) {
                Some((_, n)) => *n += 1,
                None => counts.push((ci, 1)),
            }
        }
    }
    if let Some((ci, _)) = counts.iter().max_by_key(|(_, n)| *n) {
        return (*ci).to_owned();
    }
    let unit = if own.contains('\t') || text.contains("\n\t") { "\t" } else { "  " };
    format!("{own}{unit}")
}

/// Applies one XML command and returns the new text.
pub fn apply(text: &str, cmd: &XmlCmd) -> Result<String> {
    let doc = Document::parse(text).context("the target file is not valid XML")?;
    let xpath = match cmd {
        XmlCmd::Add { xpath, .. } | XmlCmd::Replace { xpath, .. } | XmlCmd::Remove { xpath } => xpath,
    };
    let node = *select(&doc, xpath)?.first().with_context(|| format!("XPath {xpath} selects nothing"))?;
    let nl = newline(text);
    let range = node.range();
    let own_indent = leading_indent(text, range.start).unwrap_or("").to_owned();
    let mut out = String::with_capacity(text.len() + 256);
    match cmd {
        XmlCmd::Add { append, content, .. } => match append {
            Append::First | Append::Last => {
                let ci = child_indent(text, node);
                let (tag_end, self_closing) = start_tag_end(text, node);
                let block = indent_block(content, &ci, nl);
                if self_closing {
                    // <Paths/> → <Paths>…</Paths>
                    let slash = text[..tag_end - 1].rfind('/').unwrap_or(tag_end - 2);
                    out.push_str(text[..slash].trim_end());
                    out.push('>');
                    out.push_str(&format!("{nl}{block}{nl}{own_indent}</{}>", node.tag_name().name()));
                    out.push_str(&text[tag_end..]);
                } else if *append == Append::First {
                    out.push_str(&text[..tag_end]);
                    out.push_str(&format!("{nl}{block}"));
                    out.push_str(&text[tag_end..]);
                } else {
                    let close = text[..range.end].rfind("</").unwrap_or(range.end);
                    match leading_indent(text, close) {
                        Some(_) => {
                            let ls = line_start(text, close);
                            out.push_str(&text[..ls]);
                            out.push_str(&format!("{block}{nl}"));
                            out.push_str(&text[ls..]);
                        }
                        None => {
                            out.push_str(&text[..close]);
                            out.push_str(&format!("{nl}{block}{nl}{own_indent}"));
                            out.push_str(&text[close..]);
                        }
                    }
                }
            }
            Append::Before => match leading_indent(text, range.start) {
                Some(ind) => {
                    let ls = line_start(text, range.start);
                    out.push_str(&text[..ls]);
                    out.push_str(&format!("{}{nl}", indent_block(content, ind, nl)));
                    out.push_str(&text[ls..]);
                }
                None => {
                    out.push_str(&text[..range.start]);
                    out.push_str(content);
                    out.push_str(&text[range.start..]);
                }
            },
            Append::After => {
                let rest = &text[range.end..];
                let eol = rest.find('\n').map(|i| range.end + i + 1);
                match eol.filter(|&e| text[range.end..e].trim().is_empty()) {
                    Some(e) => {
                        out.push_str(&text[..e]);
                        out.push_str(&format!("{}{nl}", indent_block(content, &own_indent, nl)));
                        out.push_str(&text[e..]);
                    }
                    None => {
                        out.push_str(&text[..range.end]);
                        out.push_str(content);
                        out.push_str(&text[range.end..]);
                    }
                }
            }
        },
        XmlCmd::Replace { content, .. } => {
            let block = indent_block(content, &own_indent, nl);
            out.push_str(&text[..range.start]);
            out.push_str(block.trim_start());
            out.push_str(&text[range.end..]);
        }
        XmlCmd::Remove { .. } => {
            let rest = &text[range.end..];
            let eol = rest.find('\n').map(|i| range.end + i + 1).unwrap_or(text.len());
            if leading_indent(text, range.start).is_some() && text[range.end..eol].trim().is_empty() {
                out.push_str(&text[..line_start(text, range.start)]);
                out.push_str(&text[eol..]);
            } else {
                out.push_str(&text[..range.start]);
                out.push_str(&text[range.end..]);
            }
        }
    }
    Document::parse(&out).context("the edit would produce invalid XML")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DLCLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SMandatoryPacksData>\n\t<Paths>\n\t\t<Item>dlcpacks:/mpbeach/</Item>\n\t\t<Item>dlcpacks:/mpheist/</Item>\n\t\t<!-- comment -->\n\t\t<Item>dlcpacks:/patchday21ng/</Item>\n\t</Paths>\n</SMandatoryPacksData>\n";

    fn names(doc: &Document<'_>, xpath: &str) -> Vec<String> {
        select(doc, xpath).unwrap().iter().map(|n| string_value(*n)).collect()
    }

    #[test]
    fn selects() {
        let doc = Document::parse(DLCLIST).unwrap();
        assert_eq!(names(&doc, "/SMandatoryPacksData/Paths/Item[2]"), ["dlcpacks:/mpheist/"]);
        assert_eq!(names(&doc, "/SMandatoryPacksData/Paths/Item[.='dlcpacks:/patchday21ng/']").len(), 1);
        assert_eq!(names(&doc, "//Item[last()]"), ["dlcpacks:/patchday21ng/"]);
        assert_eq!(names(&doc, "//Item[contains(., 'heist')]/../Item[1]"), ["dlcpacks:/mpbeach/"]);
        let handling = Document::parse(
            r#"<C><H><Item type="CHandlingData"><handlingName>BUFFALO</handlingName><fMass value="1"/></Item><Item type="X"><handlingName>ADDER</handlingName><fMass value="2"/></Item></H></C>"#,
        )
        .unwrap();
        let n = select(&handling, r#"/C/H/Item[@type="CHandlingData"]/handlingName[.="BUFFALO"]/../fMass"#).unwrap();
        assert_eq!(n[0].attribute("value"), Some("1"));
        assert_eq!(
            select(&handling, "/C/H/Item[handlingName='ADDER']/fMass").unwrap()[0].attribute("value"),
            Some("2")
        );
    }

    #[test]
    fn edits_keep_formatting() {
        let add = |append, content: &str, xpath: &str| {
            apply(DLCLIST, &XmlCmd::Add { xpath: xpath.into(), append, content: content.into() }).unwrap()
        };
        let last = add(Append::Last, "<Item>dlcpacks:/mine/</Item>", "/SMandatoryPacksData/Paths");
        assert!(
            last.contains("\t\t<Item>dlcpacks:/patchday21ng/</Item>\n\t\t<Item>dlcpacks:/mine/</Item>\n\t</Paths>"),
            "{last}"
        );
        let first = add(Append::First, "<Item>top</Item>", "/SMandatoryPacksData/Paths");
        assert!(first.contains("<Paths>\n\t\t<Item>top</Item>\n\t\t<Item>dlcpacks:/mpbeach/</Item>"), "{first}");
        let before = add(Append::Before, "<Item>b</Item>", "//Item[.='dlcpacks:/mpheist/']");
        assert!(before.contains("\t\t<Item>b</Item>\n\t\t<Item>dlcpacks:/mpheist/</Item>"), "{before}");
        let after = add(Append::After, "<Item>a</Item>", "//Item[.='dlcpacks:/mpheist/']");
        assert!(after.contains("<Item>dlcpacks:/mpheist/</Item>\n\t\t<Item>a</Item>\n\t\t<!-- comment -->"), "{after}");

        let replaced =
            apply(DLCLIST, &XmlCmd::Replace { xpath: "//Item[1]".into(), content: "<Item>new</Item>".into() }).unwrap();
        assert!(replaced.contains("\t\t<Item>new</Item>\n\t\t<Item>dlcpacks:/mpheist/</Item>"));
        let removed = apply(DLCLIST, &XmlCmd::Remove { xpath: "//Item[2]".into() }).unwrap();
        assert!(!removed.contains("mpheist") && removed.contains("<!-- comment -->"));
        assert_eq!(removed.lines().count(), DLCLIST.lines().count() - 1);

        let messy = "<A>\n\t<P>\n  \t\t<I>1</I>\n\t\t<I>2</I>\n\t\t<I>3</I>\n\t</P>\n</A>";
        let added =
            apply(messy, &XmlCmd::Add { xpath: "/A/P".into(), append: Append::Last, content: "<I>4</I>".into() })
                .unwrap();
        assert!(added.contains("\t\t<I>3</I>\n\t\t<I>4</I>\n\t</P>"), "{added}");
        let empty = "<A>\n  <Paths/>\n</A>";
        let filled = apply(
            empty,
            &XmlCmd::Add { xpath: "/A/Paths".into(), append: Append::Last, content: "<Item>x</Item>".into() },
        )
        .unwrap();
        assert_eq!(filled, "<A>\n  <Paths>\n    <Item>x</Item>\n  </Paths>\n</A>");
        assert!(apply(DLCLIST, &XmlCmd::Remove { xpath: "/Nope".into() }).is_err());
    }
}
