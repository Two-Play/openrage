//! GTA IV compiled scripts (`.sco`): header and disassembler.
//!
//! A 24-byte header — magic (`SCR\x0D` plain or `scr\x0E` encrypted), code
//! size, number of static variables, number of global variables, flags,
//! signature — followed by the code, then the statics and globals (4 bytes
//! each). In `scr\x0E` scripts the code and the variables are each encrypted
//! with the archive key (AES, 16 rounds).
//!
//! Instructions are one opcode byte and its operands: jumps and calls take a
//! u32 absolute address, `ENTER` u8 parameters and u16 locals, `LEAVE` u8
//! parameters and u8 returns, `NATIVE` u8 arguments, u8 returns and the u32
//! native hash, `SWITCH` a u8 count and (u32 value, u32 address) pairs,
//! `STRING` a u8 length and the text. Opcodes from 80 push `opcode − 96`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoHeader {
    /// `SCR\x0D` or `scr\x0E`.
    pub magic: [u8; 4],
    pub code_size: u32,
    pub statics: u32,
    pub globals: u32,
    pub flags: u32,
    pub signature: u32,
}

impl ScoHeader {
    /// The magic as text, e.g. `scr 0x0E`.
    pub fn format_name(&self) -> String {
        format!("{} 0x{:02X}", String::from_utf8_lossy(&self.magic[..3]), self.magic[3])
    }
}

/// Reads the header of a script; `None` if `data` is not one.
pub fn header(data: &[u8]) -> Option<ScoHeader> {
    let magic: [u8; 4] = data.get(0..4)?.try_into().ok()?;
    if !matches!(&magic, b"SCR\x0D" | b"scr\x0E") || data.len() < 24 {
        return None;
    }
    let w = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    Some(ScoHeader { magic, code_size: w(4), statics: w(8), globals: w(12), flags: w(16), signature: w(20) })
}

/// A script's code and variables, decrypted.
#[derive(Debug, Clone)]
pub struct Sco {
    pub header: ScoHeader,
    pub code: Vec<u8>,
    pub statics: Vec<u32>,
    pub globals: Vec<u32>,
}

/// One decoded instruction.
#[derive(Debug, Clone)]
pub struct Instruction {
    pub offset: usize,
    pub name: String,
    pub operands: String,
    /// Addresses it may continue at besides the next instruction.
    pub targets: Vec<usize>,
}

/// Reads a script; `decrypt` decrypts an encrypted block in place (needed
/// for `scr\x0E` scripts).
/// Decrypts a block of an encrypted script in place.
pub type Decrypt<'a> = &'a dyn Fn(&mut [u8]);

pub fn parse(data: &[u8], decrypt: Option<Decrypt>) -> Result<Sco, String> {
    let header = header(data).ok_or("not a GTA IV script")?;
    let code_len = header.code_size as usize;
    let (statics, globals) = (header.statics as usize, header.globals as usize);
    let end = 24 + code_len + 4 * (statics + globals);
    if end > data.len() {
        return Err(format!("script needs {end} bytes, the file has {}", data.len()));
    }
    let encrypted = &header.magic == b"scr\x0E";
    let block = |from: usize, len: usize| -> Result<Vec<u8>, String> {
        let mut b = data[from..from + len].to_vec();
        if encrypted {
            (decrypt.ok_or("the script is encrypted and no key is loaded")?)(&mut b);
        }
        Ok(b)
    };
    let words = |b: Vec<u8>| b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect::<Vec<u32>>();
    let code = block(24, code_len)?;
    let statics = words(block(24 + code_len, statics * 4)?);
    let globals = words(block(24 + code_len + statics.len() * 4, globals * 4)?);
    Ok(Sco { header, code, statics, globals })
}

const NAMES: [&str; 80] = [
    "NOP",
    "ADD",
    "SUB",
    "MUL",
    "DIV",
    "MOD",
    "NOT",
    "NEG",
    "CMP_EQ",
    "CMP_NE",
    "CMP_GT",
    "CMP_GE",
    "CMP_LT",
    "CMP_LE",
    "FADD",
    "FSUB",
    "FMUL",
    "FDIV",
    "FMOD",
    "FNEG",
    "FCMP_EQ",
    "FCMP_NE",
    "FCMP_GT",
    "FCMP_GE",
    "FCMP_LT",
    "FCMP_LE",
    "VADD",
    "VSUB",
    "VMUL",
    "VDIV",
    "VNEG",
    "AND",
    "OR",
    "XOR",
    "JUMP",
    "JUMP_FALSE",
    "JUMP_TRUE",
    "I2F",
    "F2I",
    "F2V",
    "PUSH_CONST_S16",
    "PUSH_CONST_U32",
    "PUSH_CONST_F",
    "DUP",
    "DROP",
    "NATIVE",
    "CALL",
    "ENTER",
    "LEAVE",
    "LOAD",
    "STORE",
    "STORE_REV",
    "LOAD_N",
    "STORE_N",
    "LOCAL_0",
    "LOCAL_1",
    "LOCAL_2",
    "LOCAL_3",
    "LOCAL_4",
    "LOCAL_5",
    "LOCAL_6",
    "LOCAL_7",
    "LOCAL",
    "STATIC",
    "GLOBAL",
    "ARRAY",
    "SWITCH",
    "STRING",
    "NULL",
    "TEXT_LABEL_ASSIGN_STRING",
    "TEXT_LABEL_ASSIGN_INT",
    "TEXT_LABEL_APPEND_STRING",
    "TEXT_LABEL_APPEND_INT",
    "CATCH",
    "THROW",
    "TEXT_LABEL_COPY",
    "GET_PROTECT",
    "SET_PROTECT",
    "REF_PROTECT",
    "ABORT",
];

impl Sco {
    /// Decodes every instruction.
    pub fn disassemble(&self) -> Result<Vec<Instruction>, String> {
        let c = &self.code;
        let mut out = Vec::new();
        let mut pc = 0;
        while pc < c.len() {
            let op = c[pc];
            let need = |n: usize| -> Result<&[u8], String> {
                c.get(pc + 1..pc + 1 + n).ok_or_else(|| format!("instruction at {pc:#x} runs past the end"))
            };
            let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let (name, operands, targets, len): (String, String, Vec<usize>, usize) = match op {
                34..=36 | 46 => {
                    let b = need(4)?;
                    let t = u32_at(b, 0) as usize;
                    (NAMES[op as usize].into(), format!("@{t:#x}"), vec![t], 5)
                }
                40 => {
                    let b = need(2)?;
                    ("PUSH_CONST_S16".into(), i16::from_le_bytes([b[0], b[1]]).to_string(), Vec::new(), 3)
                }
                41 => ("PUSH_CONST_U32".into(), u32_at(need(4)?, 0).to_string(), Vec::new(), 5),
                42 => ("PUSH_CONST_F".into(), format!("{:?}", f32::from_bits(u32_at(need(4)?, 0))), Vec::new(), 5),
                45 => {
                    let b = need(6)?;
                    ("NATIVE".into(), format!("0x{:08X} {} {}", u32_at(b, 2), b[0], b[1]), Vec::new(), 7)
                }
                47 => {
                    let b = need(3)?;
                    ("ENTER".into(), format!("{} {}", b[0], u16::from_le_bytes([b[1], b[2]])), Vec::new(), 4)
                }
                48 => {
                    let b = need(2)?;
                    ("LEAVE".into(), format!("{} {}", b[0], b[1]), Vec::new(), 3)
                }
                66 => {
                    let count = need(1)?[0] as usize;
                    let b = need(1 + count * 8)?;
                    let cases: Vec<(u32, usize)> =
                        (0..count).map(|k| (u32_at(b, 1 + k * 8), u32_at(b, 5 + k * 8) as usize)).collect();
                    let text: Vec<String> = cases.iter().map(|(v, t)| format!("{v}:@{t:#x}")).collect();
                    ("SWITCH".into(), text.join(" "), cases.iter().map(|c| c.1).collect(), 2 + count * 8)
                }
                67 => {
                    let len = need(1)?[0] as usize;
                    let b = need(1 + len)?;
                    let text = String::from_utf8_lossy(&b[1..]).trim_end_matches('\0').to_owned();
                    ("STRING".into(), format!("\"{}\"", text.escape_default()), Vec::new(), 2 + len)
                }
                69..=72 => (NAMES[op as usize].into(), need(1)?[0].to_string(), Vec::new(), 2),
                80.. => ("PUSH_CONST".into(), (op as i32 - 96).to_string(), Vec::new(), 1),
                _ => (NAMES[op as usize].into(), String::new(), Vec::new(), 1),
            };
            out.push(Instruction { offset: pc, name, operands, targets });
            pc += len;
        }
        Ok(out)
    }

    /// The disassembly as text: functions (`ENTER`) and jump targets as labels.
    pub fn listing(&self) -> Result<String, String> {
        let code = self.disassemble()?;
        let mut labels: Vec<usize> = code.iter().flat_map(|i| i.targets.iter().copied()).collect();
        labels.sort_unstable();
        labels.dedup();
        let mut out = String::new();
        let mut functions = 0;
        for ins in &code {
            if ins.name == "ENTER" {
                out.push_str(&format!("\nfunc_{functions}:  // @{:#x}\n", ins.offset));
                functions += 1;
            } else if labels.binary_search(&ins.offset).is_ok() {
                out.push_str(&format!("  @{:#x}:\n", ins.offset));
            }
            let line = format!("    {:06x}  {:<26}{}", ins.offset, ins.name, ins.operands);
            out.push_str(line.trim_end());
            out.push('\n');
        }
        Ok(out)
    }

    /// Checks the disassembly: every jump, call and switch case lands on an
    /// instruction. Returns the number of instructions.
    pub fn check(&self) -> Result<usize, String> {
        let code = self.disassemble()?;
        let starts: Vec<usize> = code.iter().map(|i| i.offset).collect();
        for ins in &code {
            for t in &ins.targets {
                if *t != self.code.len() && starts.binary_search(t).is_err() {
                    return Err(format!("{} at {:#x} points into an instruction ({t:#x})", ins.name, ins.offset));
                }
            }
        }
        Ok(code.len())
    }

    /// Native hashes the script calls, with how often.
    pub fn natives(&self) -> Vec<(u32, usize)> {
        let mut counts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
        for ins in self.disassemble().unwrap_or_default() {
            if ins.name == "NATIVE"
                && let Some(h) = ins.operands.get(2..10).and_then(|h| u32::from_str_radix(h, 16).ok())
            {
                *counts.entry(h).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    }

    /// The strings the script pushes (`STRING`), each once, in order of first use.
    pub fn strings(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.disassemble()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.name == "STRING")
            .map(|i| i.operands.trim_matches('"').to_owned())
            .filter(|t| !t.is_empty() && seen.insert(t.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_header() {
        let mut d = b"scr\x0E".to_vec();
        for w in [14u32, 7, 0, 7, 0x7E20_4400] {
            d.extend_from_slice(&w.to_le_bytes());
        }
        let h = header(&d).unwrap();
        assert_eq!((h.code_size, h.statics, h.globals, h.signature), (14, 7, 0, 0x7E20_4400));
        assert_eq!(h.format_name(), "scr 0x0E");
        assert!(header(b"RSC\x05aaaaaaaaaaaaaaaaaaaaaaaa").is_none());
    }

    #[test]
    fn disassembles_a_plain_script() {
        // ENTER 0 2; push 3; JUMP_FALSE @0x11; NATIVE 0x2BCD1ECA 0 0; STRING "hi"; LEAVE 0 0
        let code: Vec<u8> = [
            &[47u8, 0, 2, 0][..],
            &[99],
            &[35, 0x11, 0, 0, 0],
            &[45, 0, 0, 0xCA, 0x1E, 0xCD, 0x2B],
            &[67, 3, b'h', b'i', 0],
            &[48, 0, 0],
        ]
        .concat();
        let mut d = b"SCR\x0D".to_vec();
        for w in [code.len() as u32, 1, 0, 0, 0] {
            d.extend_from_slice(&w.to_le_bytes());
        }
        d.extend_from_slice(&code);
        d.extend_from_slice(&7u32.to_le_bytes());
        let s = parse(&d, None).unwrap();
        assert_eq!(s.statics, vec![7]);
        assert_eq!(s.check(), Ok(6));
        assert_eq!(s.natives(), vec![(0x2BCD1ECA, 1)]);
        assert_eq!(s.strings(), vec!["hi"]);
        let text = s.listing().unwrap();
        assert!(text.contains("func_0:"), "{text}");
        assert!(text.contains("JUMP_FALSE                @0x11"), "{text}");
        assert!(text.contains("PUSH_CONST                3"), "{text}");
    }

    #[test]
    fn encrypted_scripts_need_a_key() {
        let mut d = b"scr\x0E".to_vec();
        for w in [1u32, 0, 0, 0, 0] {
            d.extend_from_slice(&w.to_le_bytes());
        }
        d.push(0);
        assert!(parse(&d, None).is_err());
    }
}
