//! GTA IV binary placement files (`.wpl`), shown as IPL-style text.
//!
//! Header: `u32` version (3) and 16 `u32` record counts; the records follow
//! section by section in that order. Sizes (bytes per record): `inst` 48,
//! `grge` 48, `cars` 56, `cull` 44, `strbig` 64, `lcul` 388, `zone` 24,
//! `blok` 132 (the other counts are always zero).
//!
//! `inst` (object placements): position (3 × f32), rotation quaternion
//! (x, y, z, w), model name hash, flags, LOD index (−1 for none), two more
//! values. The other sections are written field by field (numbers that read
//! as floats, others as integers, NUL-terminated text as strings), since
//! their meaning is only partly known.
//!
//! The text is lossless for numbers: floats are written in their shortest
//! exact form and always with a decimal point or exponent (`5.0`, `1e-7`),
//! integers without, so [`from_text`] turns it back into the same bytes.
//! Text fields keep only what precedes their NUL; [`from_text`] takes the
//! original records for lines that are unchanged, so leftover bytes after a
//! NUL survive as well.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WplError {
    #[error("{0}")]
    Invalid(String),
}

/// Header slot, section name and record size of every section in file order.
const SECTIONS: [(usize, &str, usize); 8] = [
    (1, "inst", 48),
    (3, "grge", 48),
    (4, "cars", 56),
    (5, "cull", 44),
    (9, "strbig", 64),
    (10, "lcul", 388),
    (11, "zone", 24),
    (16, "blok", 132),
];

const HEADER: usize = 17 * 4;

/// An object placement.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub position: [f32; 3],
    /// Quaternion `x, y, z, w` as stored: the inverse of the object's
    /// rotation (conjugate it to place the object).
    pub rotation: [f32; 4],
    pub model_hash: u32,
    pub flags: u32,
    pub lod: i32,
    pub extra: [u32; 2],
}

#[derive(Debug, Clone)]
pub struct Wpl {
    pub version: u32,
    pub instances: Vec<Instance>,
    /// The other sections: name and raw records.
    pub other: Vec<(&'static str, Vec<Vec<u8>>)>,
    /// Bytes after the last record (padding in a few files), kept as they are.
    pub trailing: Vec<u8>,
}

/// The u32 at `o`; 0 past the end (record sizes are checked before reading).
fn word(d: &[u8], o: usize) -> u32 {
    rage_core::bytes::LeRead::u32_at(d, o).unwrap_or(0)
}

fn float(d: &[u8], o: usize) -> f32 {
    f32::from_bits(word(d, o))
}

/// Whether `data` starts like a placement file.
pub fn is_wpl(data: &[u8]) -> bool {
    data.len() >= HEADER && word(data, 0) == 3
}

pub fn parse(d: &[u8]) -> Result<Wpl, WplError> {
    if !is_wpl(d) {
        return Err(WplError::Invalid("not a GTA IV placement file (version 3)".into()));
    }
    let counts: Vec<usize> = (0..17).map(|i| word(d, i * 4) as usize).collect();
    for (slot, &count) in counts.iter().enumerate().skip(1) {
        if count > 0 && !SECTIONS.iter().any(|(s, _, _)| *s == slot) {
            return Err(WplError::Invalid(format!("unknown section {slot} with {count} records")));
        }
    }
    let mut wpl = Wpl { version: counts[0] as u32, instances: Vec::new(), other: Vec::new(), trailing: Vec::new() };
    let mut o = HEADER;
    for (slot, name, size) in SECTIONS {
        let count = counts[slot];
        let end = count.checked_mul(size).and_then(|n| n.checked_add(o)).filter(|&e| e <= d.len());
        let Some(end) = end else {
            return Err(WplError::Invalid(format!("{name}: {count} records run past the end")));
        };
        if slot == 1 {
            for r in (o..end).step_by(size) {
                wpl.instances.push(Instance {
                    position: [float(d, r), float(d, r + 4), float(d, r + 8)],
                    rotation: [float(d, r + 12), float(d, r + 16), float(d, r + 20), float(d, r + 24)],
                    model_hash: word(d, r + 28),
                    flags: word(d, r + 32),
                    lod: word(d, r + 36) as i32,
                    extra: [word(d, r + 40), word(d, r + 44)],
                });
            }
        } else if count > 0 {
            wpl.other.push((name, (o..end).step_by(size).map(|r| d[r..r + size].to_vec()).collect()));
        }
        o = end;
    }
    wpl.trailing = d[o..].to_vec();
    Ok(wpl)
}

/// A float in its shortest exact form, always recognisable as a float.
fn num(v: f32) -> String {
    format!("{v:?}")
}

/// Text as a quoted field: `"` and `\\` escaped, other bytes outside
/// printable ASCII as `\\xNN`.
fn quote(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b' '..=b'~' => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    out.push('"');
    out
}

/// The bytes of a quoted field (see [`quote`]).
fn unquote(f: &str) -> Option<Result<Vec<u8>, String>> {
    let inner = f.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = Vec::new();
    let mut chars = inner.bytes();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match chars.next() {
            Some(b'x') => {
                let hex: Vec<u8> = chars.by_ref().take(2).collect();
                match std::str::from_utf8(&hex).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) if hex.len() == 2 => out.push(v),
                    _ => return Some(Err(format!("bad escape in {f}"))),
                }
            }
            Some(c @ (b'"' | b'\\')) => out.push(c),
            _ => return Some(Err(format!("bad escape in {f}"))),
        }
    }
    Some(Ok(out))
}

/// A record of a partly known section, field by field.
fn generic_fields(rec: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 4 <= rec.len() {
        // Text: at least five printable characters up to a NUL (four could be a float).
        let printable = rec[o..].iter().take_while(|b| b.is_ascii_graphic() || **b == b' ').count();
        if printable >= 5 && rec.get(o + printable) == Some(&0) {
            out.push(quote(&rec[o..o + printable]));
            o = (o + printable + 1).next_multiple_of(4);
            continue;
        }
        let w = word(rec, o);
        let f = f32::from_bits(w);
        let exp = (w >> 23) & 0xFF;
        let float_like = w & 0x7FFF_FFFF == 0 || (f.is_finite() && (100..160).contains(&exp));
        out.push(if float_like { num(f) } else { format!("{w}") });
        o += 4;
    }
    out
}

/// A `blok` record (who made a map block, and its outline): version, a
/// 92-byte text buffer (old text after the NUL is left out), flags and four
/// corner points (x, y).
fn blok_fields(rec: &[u8]) -> Vec<String> {
    let text = rec[4..96].split(|b| *b == 0).next().unwrap_or_default();
    let mut out = vec![word(rec, 0).to_string(), quote(text), word(rec, 96).to_string()];
    out.extend((0..8).map(|i| num(float(rec, 100 + i * 4))));
    out
}

/// IPL-style text; `name` gives a model's name for its hash when known.
pub fn to_text(wpl: &Wpl, name: impl Fn(u32) -> Option<String>) -> String {
    let mut out = format!("# GTA IV placement file, version {}\n", wpl.version);
    out.push_str("# inst: x, y, z, rotation x, y, z, w, model, flags, lod, extra, extra\n");
    out.push_str("# Objects name their LOD by its position in the list (lod): add new objects at the end.\ninst\n");
    for i in &wpl.instances {
        let model = name(i.model_hash).unwrap_or_else(|| format!("0x{:08X}", i.model_hash));
        let p: Vec<String> = i.position.iter().chain(&i.rotation).map(|v| num(*v)).collect();
        out.push_str(&format!("{}, {model}, {}, {}, {}, {}\n", p.join(", "), i.flags, i.lod, i.extra[0], i.extra[1]));
    }
    out.push_str("end\n");
    for (section, records) in &wpl.other {
        out.push_str(&format!("{section}\n"));
        for r in records {
            let fields = if *section == "blok" { blok_fields(r) } else { generic_fields(r) };
            out.push_str(&fields.join(", "));
            out.push('\n');
        }
        out.push_str("end\n");
    }
    out
}

/// Number of records in all sections.
pub fn record_count(wpl: &Wpl) -> usize {
    wpl.instances.len() + wpl.other.iter().map(|(_, r)| r.len()).sum::<usize>()
}

/// The bytes of a placement file.
pub fn write(wpl: &Wpl) -> Vec<u8> {
    let mut counts = [0u32; 17];
    counts[0] = wpl.version;
    counts[1] = wpl.instances.len() as u32;
    for (name, records) in &wpl.other {
        if let Some(&(slot, _, _)) = SECTIONS.iter().find(|(_, n, _)| n == name) {
            counts[slot] = records.len() as u32;
        }
    }
    let mut out: Vec<u8> = counts.iter().flat_map(|c| c.to_le_bytes()).collect();
    for &(slot, name, _) in &SECTIONS {
        if slot == 1 {
            for i in &wpl.instances {
                for v in i.position.iter().chain(&i.rotation) {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                for w in [i.model_hash, i.flags, i.lod as u32, i.extra[0], i.extra[1]] {
                    out.extend_from_slice(&w.to_le_bytes());
                }
            }
        } else if let Some((_, records)) = wpl.other.iter().find(|(n, _)| *n == name) {
            records.iter().for_each(|r| out.extend_from_slice(r));
        }
    }
    out.extend_from_slice(&wpl.trailing);
    out
}

/// The fields of a text line, split at commas outside quotes.
fn split_fields(line: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in line.chars() {
        match c {
            _ if escaped => {
                escaped = false;
                cur.push(c);
            }
            '\\' if quoted => {
                escaped = true;
                cur.push(c);
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => fields.push(std::mem::take(&mut cur).trim().to_owned()),
            _ => cur.push(c),
        }
    }
    if quoted {
        return Err("a quote is not closed".into());
    }
    fields.push(cur.trim().to_owned());
    Ok(fields)
}

fn parse_float(f: &str) -> Result<f32, String> {
    f.parse::<f32>().map_err(|_| format!("{f:?} is not a number"))
}

/// A number field as stored: a float when written as one (with `.`, an
/// exponent, `inf` or `NaN`), else an integer (negative ones as two's complement).
fn parse_word(f: &str) -> Result<u32, String> {
    let lower = f.to_ascii_lowercase();
    if lower.contains('.')
        || lower.contains("inf")
        || lower.contains("nan")
        || (lower.contains('e') && !lower.starts_with("0x"))
    {
        return parse_float(f).map(f32::to_bits);
    }
    if let Some(hex) = lower.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("{f:?} is not a hex number"));
    }
    match f.parse::<i64>() {
        Ok(v) if (i32::MIN as i64..=u32::MAX as i64).contains(&v) => Ok(v as u32),
        _ => Err(format!("{f:?} is not a number")),
    }
}

/// A record of a partly known section from its fields (see [`generic_fields`]).
fn generic_record(fields: &[String], size: usize) -> Result<Vec<u8>, String> {
    let mut rec = Vec::with_capacity(size);
    for f in fields {
        match unquote(f) {
            Some(t) => {
                rec.extend_from_slice(&t?);
                rec.push(0);
                rec.resize(rec.len().next_multiple_of(4), 0);
            }
            None => rec.extend_from_slice(&parse_word(f)?.to_le_bytes()),
        }
    }
    if rec.len() > size {
        return Err(format!("{} bytes of fields, but records of this section have {size}", rec.len()));
    }
    rec.resize(size, 0);
    Ok(rec)
}

/// A `blok` record from its fields (see [`blok_fields`]).
fn blok_record(fields: &[String]) -> Result<Vec<u8>, String> {
    if fields.len() != 11 {
        return Err(format!("a blok line has 11 fields, not {}", fields.len()));
    }
    let mut rec = vec![0u8; 132];
    rec[0..4].copy_from_slice(&parse_word(&fields[0])?.to_le_bytes());
    let text = unquote(&fields[1]).ok_or("the second field of a blok line is quoted text")??;
    if text.len() > 91 {
        return Err("the blok text is longer than 91 characters".into());
    }
    rec[4..4 + text.len()].copy_from_slice(&text);
    rec[96..100].copy_from_slice(&parse_word(&fields[2])?.to_le_bytes());
    for (i, f) in fields[3..].iter().enumerate() {
        rec[100 + i * 4..104 + i * 4].copy_from_slice(&parse_float(f)?.to_le_bytes());
    }
    Ok(rec)
}

/// Reads the text of [`to_text`] back into a placement file. Model names
/// become their hash (`hash`; `0x…` is taken as written). Lines that read
/// exactly as a record of `original` did keep that record's bytes. Errors
/// name the line.
pub fn from_text(text: &str, hash: impl Fn(&str) -> u32, original: Option<&Wpl>) -> Result<Wpl, WplError> {
    // Original records by (section, text line), to reuse unchanged ones.
    let mut known: std::collections::HashMap<(String, String), Vec<Vec<u8>>> = Default::default();
    if let Some(o) = original {
        for (section, records) in &o.other {
            for r in records {
                let fields = if *section == "blok" { blok_fields(r) } else { generic_fields(r) };
                known.entry((section.to_string(), fields.join(", "))).or_default().push(r.clone());
            }
        }
        known.values_mut().for_each(|v| v.reverse());
    }
    let mut wpl = Wpl {
        version: original.map_or(3, |o| o.version),
        instances: Vec::new(),
        other: Vec::new(),
        trailing: original.map(|o| o.trailing.clone()).unwrap_or_default(),
    };
    let mut section: Option<(&'static str, usize)> = None;
    for (n, raw) in text.lines().enumerate() {
        let err = |m: String| WplError::Invalid(format!("line {}: {m}", n + 1));
        let line = raw.trim();
        if let Some(v) = line.strip_prefix("# GTA IV placement file, version ") {
            wpl.version = v.trim().parse().map_err(|_| err(format!("{v:?} is not a version number")))?;
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, size)) = section else {
            let found = SECTIONS.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(line));
            let &(_, name, size) =
                found.ok_or_else(|| err(format!("{line:?} is not a section (inst, cars, zone, …)")))?;
            if name != "inst" && wpl.other.iter().any(|(n, _)| *n == name)
                || name == "inst" && !wpl.instances.is_empty()
            {
                return Err(err(format!("section {name} appears twice")));
            }
            if name != "inst" {
                wpl.other.push((name, Vec::new()));
            }
            section = Some((name, size));
            continue;
        };
        if line.eq_ignore_ascii_case("end") {
            section = None;
            continue;
        }
        let fields = split_fields(line).map_err(err)?;
        if name == "inst" {
            if fields.len() != 12 {
                return Err(err(format!("an inst line has 12 fields, not {}", fields.len())));
            }
            let f = |i: usize| parse_float(&fields[i]).map_err(err);
            let w = |i: usize| parse_word(&fields[i]).map_err(err);
            let model = &fields[7];
            let model_hash = match model.strip_prefix("0x").or_else(|| model.strip_prefix("0X")) {
                Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| err(format!("{model:?} is not a hash")))?,
                None if model.is_empty() => return Err(err("the model name is empty".into())),
                None => hash(model),
            };
            wpl.instances.push(Instance {
                position: [f(0)?, f(1)?, f(2)?],
                rotation: [f(3)?, f(4)?, f(5)?, f(6)?],
                model_hash,
                flags: w(8)?,
                lod: w(9)? as i32,
                extra: [w(10)?, w(11)?],
            });
            continue;
        }
        let normalized = fields.join(", ");
        let reused = known.get_mut(&(name.to_string(), normalized)).and_then(Vec::pop);
        let rec = match reused {
            Some(r) => r,
            None if name == "blok" => blok_record(&fields).map_err(err)?,
            None => generic_record(&fields, size).map_err(err)?,
        };
        wpl.other.last_mut().expect("section pushed").1.push(rec);
    }
    if let Some((name, _)) = section {
        return Err(WplError::Invalid(format!("section {name} has no end")));
    }
    // Sections in file order.
    wpl.other.sort_by_key(|(name, _)| SECTIONS.iter().position(|(_, n, _)| n == name));
    Ok(wpl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_instances_and_zones() {
        let mut d = vec![0u8; HEADER];
        d[0] = 3;
        d[4] = 1; // inst
        d[44] = 1; // zone (slot 11)
        for v in [1.5f32, -2.0, 3.0, 0.0, 0.0, 0.0, 1.0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for w in [0xABCD_EF01u32, 2, u32::MAX, 0, 0] {
            d.extend_from_slice(&w.to_le_bytes());
        }
        d.extend_from_slice(b"zone_a\0\0");
        d.extend_from_slice(&[0; 16]);
        let wpl = parse(&d).unwrap();
        assert_eq!(wpl.instances[0].position, [1.5, -2.0, 3.0]);
        assert_eq!(wpl.instances[0].lod, -1);
        let text = to_text(&wpl, |h| (h == 0xABCD_EF01).then(|| "lamp".to_owned()));
        assert!(text.contains("1.5, -2.0, 3.0, 0.0, 0.0, 0.0, 1.0, lamp, 2, -1, 0, 0\n"), "{text}");
        assert!(text.contains("zone\n\"zone_a\", 0.0, 0.0, 0.0, 0.0\nend\n"), "{text}");
        assert_eq!(record_count(&wpl), 2);
    }

    #[test]
    fn text_round_trips_and_edits() {
        let mut d = vec![0u8; HEADER];
        d[0] = 3;
        d[4] = 2; // inst
        d[64] = 1; // blok (slot 16)
        for (k, x) in [0.1f32, 123.456].iter().enumerate() {
            for v in [*x, -2.0, 3.0e-7, 0.0, 0.0, -0.70710677, 0.70710677] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            for w in [0xABCD_EF01u32 + k as u32, 2, u32::MAX, 7, 0x8000_0000] {
                d.extend_from_slice(&w.to_le_bytes());
            }
        }
        let mut blok = vec![0u8; 132];
        blok[0] = 1;
        blok[4..12].copy_from_slice(b"someone\0");
        blok[20..24].copy_from_slice(b"junk"); // left over after the NUL
        blok[100..104].copy_from_slice(&1.25f32.to_le_bytes());
        d.extend_from_slice(&blok);
        let wpl = parse(&d).unwrap();
        let name = |h: u32| (h == 0xABCD_EF01).then(|| "lamp".to_owned());
        let hash = |n: &str| if n == "lamp" { 0xABCD_EF01 } else { rage_core::hash::joaat(n) };
        let text = to_text(&wpl, name);
        // Unchanged: the same bytes, leftovers included.
        assert_eq!(write(&from_text(&text, hash, Some(&wpl)).unwrap()), d);
        // Without the original only the leftovers are lost.
        let plain = write(&from_text(&text, hash, None).unwrap());
        assert_eq!(plain[..HEADER + 96], d[..HEADER + 96]);
        // Edited: a moved object, a new one by name, the blok as it was.
        let edited = text
            .replace("123.456, ", "200.5, ")
            .replace("end\nblok", "1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, bench, 0, -1, 0, 0\nend\nblok");
        let back = from_text(&edited, hash, Some(&wpl)).unwrap();
        assert_eq!(back.instances.len(), 3);
        assert_eq!(back.instances[1].position[0], 200.5);
        assert_eq!(back.instances[2].model_hash, rage_core::hash::joaat("bench"));
        assert_eq!(back.other[0].1[0], blok);
        // Errors name the line.
        let bad = text.replace("200.5", "x").replace("123.456", "abc");
        assert!(from_text(&bad, hash, None).unwrap_err().to_string().starts_with("line 6:"));
    }
}
