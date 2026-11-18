//! Example plugin: a tool with a window of its own (a number converter) and
//! an action (hex dumps). See `plugin.toml` for how the window is described.

use openrage_plugin_sdk::file_name;
use openrage_plugin_sdk::prelude::*;

/// Largest file turned into a hex dump.
const MAX_DUMP: usize = 64 << 20;

struct HexTools;

impl Plugin for HexTools {
    fn preview(vpath: String, data: Vec<u8>) -> Result<Preview, String> {
        let Some(tool) = ToolRequest::parse(&vpath, &data) else {
            return Err("hex-tools previews no files".into());
        };
        match tool.tool.as_str() {
            "numbers" => {
                let rows = convert(tool.get("value"), tool.get("as"))?;
                let rows = rows.into_iter().map(|(k, v)| vec![k, v]).collect();
                Ok(Preview::table(vec!["Form".into(), "Value".into()], rows))
            }
            other => Err(format!("unknown tool {other:?}")),
        }
    }

    fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
        if action != "hex-dump" {
            return Err(format!("unknown action {action:?}"));
        }
        inputs
            .into_iter()
            .map(|input| {
                if input.data.len() > MAX_DUMP {
                    return Err(format!("{} is larger than {} MB", input.vpath, MAX_DUMP >> 20));
                }
                let name = format!("{}.hex.txt", file_name(&input.vpath));
                Ok(OutputFile::new(name, hex_dump(&input.data)))
            })
            .collect()
    }
}

openrage_plugin_sdk::export_plugin!(HexTools);

/// The RAGE engine's name hash (Jenkins one-at-a-time, lower case).
pub fn joaat(text: &str) -> u32 {
    let mut h: u32 = 0;
    for b in text.bytes() {
        h = h.wrapping_add(u32::from(b.to_ascii_lowercase()));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// An integer written as 0x…, 0b… or decimal (with an optional `-`).
fn parse_int(s: &str) -> Option<i128> {
    let s = s.trim().replace('_', "");
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest.to_owned()),
        None => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    let value = if let Some(hex) = lower.strip_prefix("0x") {
        i128::from_str_radix(hex, 16).ok()?
    } else if let Some(bin) = lower.strip_prefix("0b") {
        i128::from_str_radix(bin, 2).ok()?
    } else {
        lower.parse::<i128>().ok()?
    };
    Some(if neg { -value } else { value })
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// The rows of the converter for an input read as `as` (auto, integer,
/// float or text): (form, value).
pub fn convert(input: &str, read_as: &str) -> Result<Vec<(String, String)>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut row = |k: &str, v: String| rows.push((k.to_owned(), v));
    let as_int = || parse_int(input);
    let as_float = || input.parse::<f64>().ok().filter(|f| f.is_finite());
    let kind = match read_as {
        "integer" => as_int().map(|_| "integer").ok_or("not an integer (0x1F, 31, 0b11111, -1)")?,
        "float" => as_float().map(|_| "float").ok_or("not a number (1.5, -0.25, 1e3)")?,
        "text" => "text",
        _ if as_int().is_some() => "integer",
        _ if as_float().is_some() => "float",
        _ => "text",
    };
    match kind {
        "integer" => {
            let n = as_int().ok_or("not an integer")?;
            if !(i128::from(i64::MIN)..=i128::from(u64::MAX)).contains(&n) {
                return Err("outside the 64-bit range".into());
            }
            let bits = n as u64;
            row("Decimal", n.to_string());
            row("Hex", format!("0x{bits:X}"));
            if bits <= u64::from(u32::MAX) || n < 0 && n >= i128::from(i32::MIN) {
                let b32 = n as u32;
                row("Unsigned 32-bit", b32.to_string());
                row("Signed 32-bit", (b32 as i32).to_string());
                row("Bytes (little-endian)", bytes_hex(&b32.to_le_bytes()));
                row("Bytes (big-endian)", bytes_hex(&b32.to_be_bytes()));
                row("Float32 from these bits", format!("{}", f32::from_bits(b32)));
                row("Binary", format!("{b32:032b}"));
            } else {
                row("Unsigned 64-bit", bits.to_string());
                row("Signed 64-bit", (bits as i64).to_string());
                row("Bytes (little-endian)", bytes_hex(&bits.to_le_bytes()));
                row("Bytes (big-endian)", bytes_hex(&bits.to_be_bytes()));
                row("Float64 from these bits", format!("{}", f64::from_bits(bits)));
                row("Binary", format!("{bits:064b}"));
            }
        }
        "float" => {
            let f = as_float().ok_or("not a number")?;
            let f32_bits = (f as f32).to_bits();
            row("Float32", format!("{}", f as f32));
            row("Float32 bits", format!("0x{f32_bits:08X}"));
            row("Float32 bytes (little-endian)", bytes_hex(&f32_bits.to_le_bytes()));
            row("Float64 bits", format!("0x{:016X}", f.to_bits()));
            row("Float64 bytes (little-endian)", bytes_hex(&f.to_bits().to_le_bytes()));
        }
        _ => {
            row("Text bytes (UTF-8)", bytes_hex(input.as_bytes()));
            row("Length", format!("{} bytes", input.len()));
        }
    }
    // Every input is also a possible name: its hash, as the game stores it.
    let h = joaat(input);
    row("joaat (RAGE hash)", format!("0x{h:08X}"));
    row("joaat unsigned / signed", format!("{h} / {}", h as i32));
    Ok(rows)
}

/// A classic hex dump: offset, 16 bytes, and the printable characters.
pub fn hex_dump(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 4 + 64);
    for (i, chunk) in data.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let ascii: String = chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
        out.push_str(&format!("{:08X}  {:<47}  {ascii}\n", i * 16, hex.join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(rows: &'a [(String, String)], key: &str) -> &'a str {
        rows.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or_else(|| panic!("no {key}: {rows:?}"))
    }

    #[test]
    fn converts_numbers_and_names() {
        let r = convert("0x1F", "auto").unwrap();
        assert_eq!(value(&r, "Decimal"), "31");
        assert_eq!(value(&r, "Bytes (little-endian)"), "1F 00 00 00");
        let r = convert("-1", "auto").unwrap();
        assert_eq!(value(&r, "Hex"), "0xFFFFFFFFFFFFFFFF");
        assert_eq!(value(&r, "Unsigned 32-bit"), "4294967295");
        let r = convert("1.5", "auto").unwrap();
        assert_eq!(value(&r, "Float32 bits"), "0x3FC00000");
        // The game's own hash of "adder" (also what `openrage search adder` prints).
        assert_eq!(joaat("adder"), 0xB779A091);
        assert_eq!(value(&convert("Adder", "text").unwrap(), "joaat (RAGE hash)"), "0xB779A091");
        assert!(convert("hello", "integer").is_err());
        assert!(convert("", "auto").unwrap().is_empty());
    }

    #[test]
    fn dumps_bytes() {
        assert_eq!(hex_dump(b"AB\x00"), format!("00000000  {:<47}  AB.\n", "41 42 00"));
    }
}
