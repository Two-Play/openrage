//! RBF files (`RBF0`): a compact binary XML used by a few `.ymt` files
//! (e.g. the movie subtitles in `moviesubs.rpf`).
//!
//! A stream of records, little-endian. Each record starts with a descriptor
//! index and a data type; a new index is followed by the element name
//! (u16 length + ASCII). Types: `0x00` element (u16, u16, attribute count),
//! `0x10` u32, `0x20`/`0x30` true/false, `0x40` float, `0x50` three floats,
//! `0x60` string (u16 length + ASCII). `FD FF` + i32 length + bytes is element
//! content; `FF FF` closes the current element. The first `n` values after an
//! element (its attribute count) are its attributes.

use crate::xml::{self, Writer};

pub const MAGIC: &[u8; 4] = b"RBF0";

#[derive(Debug, thiserror::Error)]
#[error("invalid RBF file: {0}")]
pub struct RbfError(String);

#[derive(Debug, Clone)]
pub enum Value {
    Element(Element),
    Uint(String, u32),
    Bool(String, bool),
    Float(String, f32),
    Float3(String, [f32; 3]),
    Str(String, String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<Value>,
    pub children: Vec<Value>,
    pending: usize,
}

impl Element {
    fn add(&mut self, v: Value) {
        if self.pending > 0 {
            self.pending -= 1;
            self.attributes.push(v);
        } else {
            self.children.push(v);
        }
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find_map(|a| match a {
            Value::Str(n, v) if n == name => Some(v.as_str()),
            _ => None,
        })
    }
}

pub fn is_rbf(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], RbfError> {
        let s = self.d.get(self.pos..self.pos + n).ok_or_else(|| RbfError("unexpected end of file".into()))?;
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, RbfError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, RbfError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap_or_default()))
    }
    fn u32(&mut self) -> Result<u32, RbfError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap_or_default()))
    }
    fn f32(&mut self) -> Result<f32, RbfError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn string(&mut self) -> Result<String, RbfError> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// Parses the file into its root element.
pub fn parse(data: &[u8]) -> Result<Element, RbfError> {
    if !is_rbf(data) {
        return Err(RbfError("missing RBF0 header".into()));
    }
    let mut r = Reader { d: data, pos: 4 };
    let mut names: Vec<String> = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    while r.pos < data.len() {
        let index = r.u8()?;
        match index {
            0xFF => {
                if r.u8()? != 0xFF {
                    return Err(RbfError("bad close tag".into()));
                }
                let done = stack.pop().ok_or_else(|| RbfError("close tag without element".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.add(Value::Element(done)),
                    None => return Ok(done),
                }
            }
            0xFD => {
                if r.u8()? != 0xFF {
                    return Err(RbfError("bad content marker".into()));
                }
                let n = r.u32()? as usize;
                let bytes = r.take(n)?.to_vec();
                stack
                    .last_mut()
                    .ok_or_else(|| RbfError("content outside element".into()))?
                    .children
                    .push(Value::Bytes(bytes));
            }
            _ => {
                let kind = r.u8()?;
                if index as usize == names.len() {
                    names.push(r.string()?);
                }
                let name =
                    names.get(index as usize).cloned().ok_or_else(|| RbfError(format!("unknown name {index}")))?;
                let value = match kind {
                    0x00 => {
                        let _ = (r.u16()?, r.u16()?);
                        let pending = r.u16()? as usize;
                        stack.push(Element { name, pending, ..Default::default() });
                        continue;
                    }
                    0x10 => Value::Uint(name, r.u32()?),
                    0x20 => Value::Bool(name, true),
                    0x30 => Value::Bool(name, false),
                    0x40 => Value::Float(name, r.f32()?),
                    0x50 => Value::Float3(name, [r.f32()?, r.f32()?, r.f32()?]),
                    0x60 => Value::Str(name, r.string()?),
                    t => return Err(RbfError(format!("unsupported data type {t:#04x}"))),
                };
                stack.last_mut().ok_or_else(|| RbfError("value outside element".into()))?.add(value);
            }
        }
    }
    Err(RbfError("unexpected end of file".into()))
}

/// Parses the file and writes it as XML (CodeWalker-style).
pub fn to_xml(data: &[u8]) -> Result<(String, Vec<String>, String), RbfError> {
    let root = parse(data)?;
    let mut w = Writer::new();
    write(&mut w, 0, &root);
    let (text, errors) = w.finish();
    Ok((text, errors, root.name))
}

fn bool_text(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

fn write(w: &mut Writer, level: usize, e: &Element) {
    let mut tag = e.name.clone();
    for a in &e.attributes {
        let (name, value) = match a {
            Value::Str(n, v) => (n, xml::escape(v)),
            Value::Float(n, v) => (n, xml::float(*v)),
            Value::Uint(n, v) => (n, v.to_string()),
            Value::Bool(n, v) => (n, bool_text(*v).to_owned()),
            _ => continue,
        };
        tag.push_str(&format!(" {name}=\"{value}\""));
    }
    if e.children.is_empty() {
        return w.empty(level, &tag);
    }
    // A single text child stays on one line.
    if let ([Value::Bytes(b)], true) = (e.children.as_slice(), e.attributes.is_empty()) {
        let text = String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).into_owned();
        return w.text(level, &tag, &text);
    }
    w.open(level, &tag);
    let inner = level + 1;
    for child in &e.children {
        match child {
            Value::Bytes(b) => match e.attribute("content") {
                Some("char_array") => {
                    let v: Vec<String> = b.iter().map(|x| x.to_string()).collect();
                    w.raw_array_lines(inner, &v);
                }
                Some("short_array") => {
                    let v: Vec<String> =
                        b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c).to_string()).collect();
                    w.raw_array_lines(inner, &v);
                }
                Some(other) => w.error(inner, format!("unexpected content type {other}")),
                None => {
                    let text = String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).into_owned();
                    w.raw_array_lines(inner, &[xml::escape(&text)]);
                }
            },
            Value::Element(c) => write(w, inner, c),
            Value::Uint(n, v) => w.value(inner, n, &format!("0x{v:08X}")),
            Value::Bool(n, v) => w.value(inner, n, bool_text(*v)),
            Value::Float(n, v) => w.value(inner, n, &xml::float(*v)),
            Value::Float3(n, v) => w.empty(
                inner,
                &format!("{n} x=\"{}\" y=\"{}\" z=\"{}\"", xml::float(v[0]), xml::float(v[1]), xml::float(v[2])),
            ),
            Value::Str(n, v) => w.string(inner, n, v),
        }
    }
    w.close(level, &e.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_elements() {
        let mut d = b"RBF0".to_vec();
        // <root a="x"> <v value=…/> "hi" </root>
        d.extend([0, 0, 4, 0, b'r', b'o', b'o', b't', 0, 0, 0, 0, 1, 0]);
        d.extend([1, 0x60, 1, 0, b'a', 1, 0, b'x']);
        d.extend([2, 0x40, 1, 0, b'v']);
        d.extend(1.5f32.to_le_bytes());
        d.extend([0xFF, 0xFF]);
        let root = parse(&d).unwrap();
        assert_eq!(root.name, "root");
        assert_eq!(root.attribute("a"), Some("x"));
        let (text, errors, _) = to_xml(&d).unwrap();
        assert!(errors.is_empty());
        assert!(text.contains("<root a=\"x\">\n <v value=\"1.5\" />\n</root>"), "{text}");
    }
}
