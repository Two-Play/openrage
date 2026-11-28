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
//!
//! GTA VI: most files have `FB FF 01 00` after the magic, and with it
//! integers and floats of 8 bytes instead of 4.

use crate::xml::{self, Writer};

pub const MAGIC: &[u8; 4] = b"RBF0";

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RbfError {
    #[error("invalid RBF file: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub enum Value {
    Element(Element),
    Uint(String, u32),
    /// GTA VI: an 8-byte integer.
    Uint64(String, u64),
    /// GTA VI: an 8-byte float.
    Float64(String, f64),
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
        let s = self.d.get(self.pos..self.pos + n).ok_or_else(|| RbfError::Invalid("unexpected end of file".into()))?;
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
    fn u64(&mut self) -> Result<u64, RbfError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap_or_default()))
    }
    fn f64(&mut self) -> Result<f64, RbfError> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn string(&mut self) -> Result<String, RbfError> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// Descriptor index that closes the current element (`FF FF`).
const INDEX_CLOSE: u8 = 0xFF;
/// Descriptor index that introduces element content (`FD FF`).
const INDEX_CONTENT: u8 = 0xFD;
/// Second byte of the close and content markers.
const MARKER_TAIL: u8 = 0xFF;

const TYPE_ELEMENT: u8 = 0x00;
const TYPE_UINT: u8 = 0x10;
const TYPE_TRUE: u8 = 0x20;
const TYPE_FALSE: u8 = 0x30;
const TYPE_FLOAT: u8 = 0x40;
const TYPE_FLOAT3: u8 = 0x50;
const TYPE_STRING: u8 = 0x60;

struct Parser<'a> {
    r: Reader<'a>,
    wide: bool,
    names: Vec<String>,
    stack: Vec<Element>,
}

impl Parser<'_> {
    /// Closes the current element; the root element, once closed, is returned.
    fn close(&mut self) -> Result<Option<Element>, RbfError> {
        if self.r.u8()? != MARKER_TAIL {
            return Err(RbfError::Invalid("bad close tag".into()));
        }
        let done = self.stack.pop().ok_or_else(|| RbfError::Invalid("close tag without element".into()))?;
        match self.stack.last_mut() {
            Some(parent) => {
                parent.add(Value::Element(done));
                Ok(None)
            }
            None => Ok(Some(done)),
        }
    }

    fn content(&mut self) -> Result<(), RbfError> {
        if self.r.u8()? != MARKER_TAIL {
            return Err(RbfError::Invalid("bad content marker".into()));
        }
        let n = self.r.u32()? as usize;
        let bytes = self.r.take(n)?.to_vec();
        self.stack
            .last_mut()
            .ok_or_else(|| RbfError::Invalid("content outside element".into()))?
            .children
            .push(Value::Bytes(bytes));
        Ok(())
    }

    /// One named record: an element start or a value for the current element.
    fn record(&mut self, index: u8) -> Result<(), RbfError> {
        let kind = self.r.u8()?;
        if index as usize == self.names.len() {
            let name = self.r.string()?;
            self.names.push(name);
        }
        let name = self
            .names
            .get(index as usize)
            .cloned()
            .ok_or_else(|| RbfError::Invalid(format!("unknown name {index}")))?;
        let Some(value) = self.value(kind, name)? else { return Ok(()) };
        self.stack.last_mut().ok_or_else(|| RbfError::Invalid("value outside element".into()))?.add(value);
        Ok(())
    }

    /// The value of a record, or `None` when it opened an element instead.
    fn value(&mut self, kind: u8, name: String) -> Result<Option<Value>, RbfError> {
        let value = match kind {
            TYPE_ELEMENT => {
                self.open_element(name)?;
                return Ok(None);
            }
            TYPE_UINT | TYPE_FLOAT | TYPE_FLOAT3 => self.number(kind, name)?,
            TYPE_TRUE => Value::Bool(name, true),
            TYPE_FALSE => Value::Bool(name, false),
            TYPE_STRING => Value::Str(name, self.r.string()?),
            t => return Err(RbfError::Invalid(format!("unsupported data type {t:#04x}"))),
        };
        Ok(Some(value))
    }

    fn open_element(&mut self, name: String) -> Result<(), RbfError> {
        let _ = (self.r.u16()?, self.r.u16()?);
        let pending = self.r.u16()? as usize;
        self.stack.push(Element { name, pending, ..Default::default() });
        Ok(())
    }

    fn number(&mut self, kind: u8, name: String) -> Result<Value, RbfError> {
        if self.wide { self.wide_number(kind, name) } else { self.narrow_number(kind, name) }
    }

    /// GTA VI: integers and floats of 8 bytes.
    fn wide_number(&mut self, kind: u8, name: String) -> Result<Value, RbfError> {
        let r = &mut self.r;
        Ok(match kind {
            TYPE_UINT => Value::Uint64(name, r.u64()?),
            TYPE_FLOAT => Value::Float64(name, r.f64()?),
            _ => Value::Float3(name, [r.f64()? as f32, r.f64()? as f32, r.f64()? as f32]),
        })
    }

    fn narrow_number(&mut self, kind: u8, name: String) -> Result<Value, RbfError> {
        let r = &mut self.r;
        Ok(match kind {
            TYPE_UINT => Value::Uint(name, r.u32()?),
            TYPE_FLOAT => Value::Float(name, r.f32()?),
            _ => Value::Float3(name, [r.f32()?, r.f32()?, r.f32()?]),
        })
    }

    /// Handles one record; the root element, once closed, is returned.
    fn step(&mut self) -> Result<Option<Element>, RbfError> {
        let index = self.r.u8()?;
        match index {
            INDEX_CLOSE => return self.close(),
            INDEX_CONTENT => self.content()?,
            _ => self.record(index)?,
        }
        Ok(None)
    }
}

/// Parses the file into its root element.
pub fn parse(data: &[u8]) -> Result<Element, RbfError> {
    if !is_rbf(data) {
        return Err(RbfError::Invalid("missing RBF0 header".into()));
    }
    // GTA VI: with these four bytes, numbers take 8 bytes.
    let wide = data.get(4..8) == Some(&[0xFB, 0xFF, 0x01, 0x00]);
    let r = Reader { d: data, pos: if wide { 8 } else { 4 } };
    let mut p = Parser { r, wide, names: Vec::new(), stack: Vec::new() };
    while p.r.pos < data.len() {
        if let Some(root) = p.step()? {
            return Ok(root);
        }
    }
    Err(RbfError::Invalid("unexpected end of file".into()))
}

/// Parses the file and writes it as XML (CodeWalker-style).
pub fn to_xml(data: &[u8]) -> Result<(String, Vec<String>, String), RbfError> {
    let root = parse(data)?;
    let mut w = Writer::new();
    write(&mut w, 0, &root);
    let (text, errors) = w.finish();
    Ok((text, errors, root.name))
}

/// An 8-byte float as text: like a 4-byte one when it is one exactly.
fn float64(v: f64) -> String {
    if f64::from(v as f32) == v { xml::float(v as f32) } else { v.to_string() }
}

/// An 8-byte integer as text: small ones as numbers, the rest (hashes) in hex.
fn uint64(v: u64) -> String {
    if v < 1 << 31 { v.to_string() } else { format!("0x{v:016X}") }
}

fn bool_text(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// The attribute as `(name, text)`; other kinds are not attributes.
fn attribute_text(a: &Value) -> Option<(&String, String)> {
    Some(match a {
        Value::Str(n, v) => (n, xml::escape(v)),
        Value::Float(n, v) => (n, xml::float(*v)),
        Value::Uint(n, v) => (n, v.to_string()),
        Value::Uint64(n, v) => (n, uint64(*v)),
        Value::Float64(n, v) => (n, float64(*v)),
        Value::Bool(n, v) => (n, bool_text(*v).to_owned()),
        _ => return None,
    })
}

/// Floats per row for the float-list content types.
fn floats_per_row(kind: &str) -> usize {
    match kind {
        "matrix34" | "vector3_array" => 3,
        "float_array" => 1,
        "vector2_array" | "vec2v_array" => 2,
        _ => 4,
    }
}

/// Text content: the bytes up to a trailing NUL.
fn content_text(b: &[u8]) -> String {
    String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).into_owned()
}

fn write_content(w: &mut Writer, inner: usize, e: &Element, b: &[u8]) {
    match e.attribute("content") {
        Some("char_array") => {
            let v: Vec<String> = b.iter().map(|x| x.to_string()).collect();
            w.raw_array_lines(inner, &v);
        }
        Some("short_array") => {
            let v: Vec<String> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c).to_string()).collect();
            w.raw_array_lines(inner, &v);
        }
        // Matrices and float lists: little-endian floats, a row per line
        // (matrix34: the three axes and the position).
        Some(
            kind @ ("matrix34" | "matrix44" | "float_array" | "vector2_array" | "vec2v_array" | "vector3_array"
            | "vector4_array"),
        ) => {
            let floats: Vec<String> = b.as_chunks::<4>().0.iter().map(|c| xml::float(f32::from_le_bytes(*c))).collect();
            let rows: Vec<String> = floats.chunks(floats_per_row(kind)).map(|r| r.join(", ")).collect();
            w.raw_array_lines(inner, &rows);
        }
        Some(other) => w.error(inner, format!("unexpected content type {other}")),
        None => w.raw_array_lines(inner, &[xml::escape(&content_text(b))]),
    }
}

fn write_child(w: &mut Writer, inner: usize, e: &Element, child: &Value) {
    match child {
        Value::Bytes(b) => write_content(w, inner, e, b),
        Value::Element(c) => write(w, inner, c),
        Value::Uint(n, v) => w.value(inner, n, &format!("0x{v:08X}")),
        Value::Uint64(n, v) => w.value(inner, n, &uint64(*v)),
        Value::Float64(n, v) => w.value(inner, n, &float64(*v)),
        Value::Bool(n, v) => w.value(inner, n, bool_text(*v)),
        Value::Float(n, v) => w.value(inner, n, &xml::float(*v)),
        Value::Float3(n, v) => w.empty(
            inner,
            &format!("{n} x=\"{}\" y=\"{}\" z=\"{}\"", xml::float(v[0]), xml::float(v[1]), xml::float(v[2])),
        ),
        Value::Str(n, v) => w.string(inner, n, v),
    }
}

fn write(w: &mut Writer, level: usize, e: &Element) {
    let mut tag = e.name.clone();
    for (name, value) in e.attributes.iter().filter_map(attribute_text) {
        tag.push_str(&format!(" {name}=\"{value}\""));
    }
    if e.children.is_empty() {
        return w.empty(level, &tag);
    }
    // A single text child stays on one line.
    if let ([Value::Bytes(b)], true) = (e.children.as_slice(), e.attributes.is_empty()) {
        return w.text(level, &tag, &content_text(b));
    }
    w.open(level, &tag);
    let inner = level + 1;
    for child in &e.children {
        write_child(w, inner, e, child);
    }
    w.close(level, &e.name);
}

#[cfg(test)]
mod format_tests;

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

    /// GTA VI: 8-byte numbers after the four bytes that announce them.
    #[test]
    fn parses_wide_numbers() {
        let mut d = b"RBF0\xFB\xFF\x01\x00".to_vec();
        d.extend([0, 0, 1, 0, b'r', 0, 0, 0, 0, 1, 0]);
        d.extend([1, 0x10, 1, 0, b'n']);
        d.extend(7u64.to_le_bytes());
        d.extend([2, 0x40, 1, 0, b'v']);
        d.extend(0.25f64.to_le_bytes());
        // The same name with another type, as GTA VI's files have it.
        d.extend([2, 0x10]);
        d.extend(u64::MAX.to_le_bytes());
        d.extend([0xFF, 0xFF]);
        let (text, errors, _) = to_xml(&d).unwrap();
        assert!(errors.is_empty());
        assert!(
            text.contains("<r n=\"7\">\n <v value=\"0.25\" />\n <v value=\"0xFFFFFFFFFFFFFFFF\" />\n</r>"),
            "{text}"
        );
    }
}
