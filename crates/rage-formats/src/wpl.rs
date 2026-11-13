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

#[derive(Debug)]
pub struct WplError(pub String);

impl std::fmt::Display for WplError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WplError {}

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
    /// Quaternion `x, y, z, w`.
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
}

fn word(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
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
        return Err(WplError("not a GTA IV placement file (version 3)".into()));
    }
    let counts: Vec<usize> = (0..17).map(|i| word(d, i * 4) as usize).collect();
    for (slot, &count) in counts.iter().enumerate().skip(1) {
        if count > 0 && !SECTIONS.iter().any(|(s, _, _)| *s == slot) {
            return Err(WplError(format!("unknown section {slot} with {count} records")));
        }
    }
    let mut wpl = Wpl { version: counts[0] as u32, instances: Vec::new(), other: Vec::new() };
    let mut o = HEADER;
    for (slot, name, size) in SECTIONS {
        let count = counts[slot];
        let end = count.checked_mul(size).and_then(|n| n.checked_add(o)).filter(|&e| e <= d.len());
        let Some(end) = end else {
            return Err(WplError(format!("{name}: {count} records run past the end")));
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
    Ok(wpl)
}

fn num(v: f32) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    s.strip_suffix('.').unwrap_or(s).to_owned()
}

/// A record of a partly known section, field by field.
fn generic_fields(rec: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 4 <= rec.len() {
        // Text: at least five printable characters up to a NUL (four could be a float).
        let printable = rec[o..].iter().take_while(|b| b.is_ascii_graphic() || **b == b' ').count();
        if printable >= 5 && rec.get(o + printable) == Some(&0) {
            out.push(format!("\"{}\"", String::from_utf8_lossy(&rec[o..o + printable])));
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
    let mut out =
        vec![word(rec, 0).to_string(), format!("\"{}\"", String::from_utf8_lossy(text)), word(rec, 96).to_string()];
    out.extend((0..8).map(|i| num(float(rec, 100 + i * 4))));
    out
}

/// IPL-style text; `name` gives a model's name for its hash when known.
pub fn to_text(wpl: &Wpl, name: impl Fn(u32) -> Option<String>) -> String {
    let mut out = format!("# GTA IV placement file, version {}\n", wpl.version);
    out.push_str("# inst: x, y, z, rotation x, y, z, w, model, flags, lod, extra, extra\ninst\n");
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
        assert!(text.contains("1.5, -2, 3, 0, 0, 0, 1, lamp, 2, -1, 0, 0\n"), "{text}");
        assert!(text.contains("zone\n\"zone_a\", 0, 0, 0, 0\nend\n"), "{text}");
        assert_eq!(record_count(&wpl), 2);
    }
}
