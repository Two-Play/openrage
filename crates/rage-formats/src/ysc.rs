//! Compiled game scripts (`.ysc`, `rage::scrProgram`): header, strings,
//! natives and a disassembler.
//!
//! Header (resource version 10): code pages `0x10` (pointers to 16 KiB pages),
//! code length `0x1C`, parameter count `0x20`, statics count `0x24`, natives
//! count `0x2C`, statics `0x30`, natives `0x40` (u64 hashes, each rotated left
//! by `(code length + index) % 64`), name hash `0x58`, name `0x60`, string
//! pages `0x68`, string size `0x70`.

use rage_resource::{Resource, ResourceError};

const PAGE: usize = 0x4000;

/// Operand layout of an opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operands {
    None,
    U8,
    U8x2,
    U8x3,
    U16,
    S16,
    U24,
    U32,
    F32,
    /// `NATIVE`: u8 (argument/return counts), u16 big-endian native index.
    Native,
    /// `ENTER`: u8 parameters, u16 locals, u8 name length, name.
    Enter,
    /// `LEAVE`: u8 parameters, u8 returns.
    Leave,
    /// `SWITCH`: u8 count, then count × (u32 value, i16 jump).
    Switch,
    /// Jumps: i16 relative to the next instruction.
    Jump,
    /// `CALL`: u24 absolute address.
    Call,
}

use Operands as O;

/// Opcode tables: newer game builds added `STATIC_U24*` (which shifts every
/// later opcode) and `IS_BIT_SET`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// Without `STATIC_U24*` and `IS_BIT_SET`.
    Classic,
    /// With them (later updates, GTA V Enhanced).
    Current,
}

fn table(t: Table) -> Vec<(&'static str, Operands)> {
    OPCODES
        .iter()
        .copied()
        .filter(|(n, _)| t == Table::Current || !(n.starts_with("STATIC_U24") || *n == "IS_BIT_SET"))
        .collect()
}

/// GTA V's opcodes (PC), in order (the current table).
const OPCODES: &[(&str, Operands)] = &[
    ("NOP", O::None),
    ("IADD", O::None),
    ("ISUB", O::None),
    ("IMUL", O::None),
    ("IDIV", O::None),
    ("IMOD", O::None),
    ("INOT", O::None),
    ("INEG", O::None),
    ("IEQ", O::None),
    ("INE", O::None),
    ("IGT", O::None),
    ("IGE", O::None),
    ("ILT", O::None),
    ("ILE", O::None),
    ("FADD", O::None),
    ("FSUB", O::None),
    ("FMUL", O::None),
    ("FDIV", O::None),
    ("FMOD", O::None),
    ("FNEG", O::None),
    ("FEQ", O::None),
    ("FNE", O::None),
    ("FGT", O::None),
    ("FGE", O::None),
    ("FLT", O::None),
    ("FLE", O::None),
    ("VADD", O::None),
    ("VSUB", O::None),
    ("VMUL", O::None),
    ("VDIV", O::None),
    ("VNEG", O::None),
    ("IAND", O::None),
    ("IOR", O::None),
    ("IXOR", O::None),
    ("I2F", O::None),
    ("F2I", O::None),
    ("F2V", O::None),
    ("PUSH_CONST_U8", O::U8),
    ("PUSH_CONST_U8_U8", O::U8x2),
    ("PUSH_CONST_U8_U8_U8", O::U8x3),
    ("PUSH_CONST_U32", O::U32),
    ("PUSH_CONST_F", O::F32),
    ("DUP", O::None),
    ("DROP", O::None),
    ("NATIVE", O::Native),
    ("ENTER", O::Enter),
    ("LEAVE", O::Leave),
    ("LOAD", O::None),
    ("STORE", O::None),
    ("STORE_REV", O::None),
    ("LOAD_N", O::None),
    ("STORE_N", O::None),
    ("ARRAY_U8", O::U8),
    ("ARRAY_U8_LOAD", O::U8),
    ("ARRAY_U8_STORE", O::U8),
    ("LOCAL_U8", O::U8),
    ("LOCAL_U8_LOAD", O::U8),
    ("LOCAL_U8_STORE", O::U8),
    ("STATIC_U8", O::U8),
    ("STATIC_U8_LOAD", O::U8),
    ("STATIC_U8_STORE", O::U8),
    ("IADD_U8", O::U8),
    ("IMUL_U8", O::U8),
    ("IOFFSET", O::None),
    ("IOFFSET_U8", O::U8),
    ("IOFFSET_U8_LOAD", O::U8),
    ("IOFFSET_U8_STORE", O::U8),
    ("PUSH_CONST_S16", O::S16),
    ("IADD_S16", O::S16),
    ("IMUL_S16", O::S16),
    ("IOFFSET_S16", O::S16),
    ("IOFFSET_S16_LOAD", O::S16),
    ("IOFFSET_S16_STORE", O::S16),
    ("ARRAY_U16", O::U16),
    ("ARRAY_U16_LOAD", O::U16),
    ("ARRAY_U16_STORE", O::U16),
    ("LOCAL_U16", O::U16),
    ("LOCAL_U16_LOAD", O::U16),
    ("LOCAL_U16_STORE", O::U16),
    ("STATIC_U16", O::U16),
    ("STATIC_U16_LOAD", O::U16),
    ("STATIC_U16_STORE", O::U16),
    ("GLOBAL_U16", O::U16),
    ("GLOBAL_U16_LOAD", O::U16),
    ("GLOBAL_U16_STORE", O::U16),
    ("J", O::Jump),
    ("JZ", O::Jump),
    ("IEQ_JZ", O::Jump),
    ("INE_JZ", O::Jump),
    ("IGT_JZ", O::Jump),
    ("IGE_JZ", O::Jump),
    ("ILT_JZ", O::Jump),
    ("ILE_JZ", O::Jump),
    ("CALL", O::Call),
    ("STATIC_U24", O::U24),
    ("STATIC_U24_LOAD", O::U24),
    ("STATIC_U24_STORE", O::U24),
    ("GLOBAL_U24", O::U24),
    ("GLOBAL_U24_LOAD", O::U24),
    ("GLOBAL_U24_STORE", O::U24),
    ("PUSH_CONST_U24", O::U24),
    ("SWITCH", O::Switch),
    ("STRING", O::None),
    ("STRINGHASH", O::None),
    ("TEXT_LABEL_ASSIGN_STRING", O::U8),
    ("TEXT_LABEL_ASSIGN_INT", O::U8),
    ("TEXT_LABEL_APPEND_STRING", O::U8),
    ("TEXT_LABEL_APPEND_INT", O::U8),
    ("TEXT_LABEL_COPY", O::None),
    ("CATCH", O::None),
    ("THROW", O::None),
    ("CALLINDIRECT", O::None),
    ("PUSH_CONST_M1", O::None),
    ("PUSH_CONST_0", O::None),
    ("PUSH_CONST_1", O::None),
    ("PUSH_CONST_2", O::None),
    ("PUSH_CONST_3", O::None),
    ("PUSH_CONST_4", O::None),
    ("PUSH_CONST_5", O::None),
    ("PUSH_CONST_6", O::None),
    ("PUSH_CONST_7", O::None),
    ("PUSH_CONST_FM1", O::None),
    ("PUSH_CONST_F0", O::None),
    ("PUSH_CONST_F1", O::None),
    ("PUSH_CONST_F2", O::None),
    ("PUSH_CONST_F3", O::None),
    ("PUSH_CONST_F4", O::None),
    ("PUSH_CONST_F5", O::None),
    ("PUSH_CONST_F6", O::None),
    ("PUSH_CONST_F7", O::None),
    ("IS_BIT_SET", O::None),
];

/// A compiled script.
#[derive(Debug, Clone, Default)]
pub struct Script {
    pub name: String,
    pub name_hash: u32,
    pub code: Vec<u8>,
    pub parameters: u32,
    pub statics: Vec<i64>,
    /// Native function hashes, as called by `NATIVE` (index into this list).
    pub natives: Vec<u64>,
    /// The string table (concatenated pages); `STRING` takes offsets into it.
    pub strings: Vec<u8>,
    /// The opcode table the code decodes with (see [`Script::detect_table`]).
    pub table: Option<Table>,
}

/// One decoded instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct Instruction {
    pub offset: usize,
    pub name: &'static str,
    /// Operands as text (`12`, `@0x1A2B`, `native 0x…`, …).
    pub operands: String,
    /// Jump or call targets (code offsets), for checking and labels.
    pub targets: Vec<usize>,
    /// For `ENTER`: the function's name if the compiler kept it.
    pub function: Option<String>,
}

fn pages(res: &Resource, table: u64, size: usize) -> Result<Vec<u8>, ResourceError> {
    let mut out = Vec::with_capacity(size);
    let mut i = 0;
    while out.len() < size {
        let ptr = res.u64(table + i * 8)?;
        let n = (size - out.len()).min(PAGE);
        out.extend_from_slice(res.bytes(ptr, n)?);
        i += 1;
    }
    Ok(out)
}

pub fn parse(res: &Resource) -> Result<Script, ResourceError> {
    let root = Resource::ROOT;
    let code_len = res.u32(root + 0x1C)? as usize;
    let statics_count = res.u32(root + 0x24)? as u64;
    let natives_count = res.u32(root + 0x2C)? as u64;
    let strings_len = res.u32(root + 0x70)? as usize;
    if code_len > 64 << 20 || natives_count > 1 << 16 || statics_count > 1 << 20 || strings_len > 64 << 20 {
        return Err(ResourceError::Format(format!("implausible script header (code {code_len} bytes)")));
    }
    let name_ptr = res.u64(root + 0x60)?;
    let statics_at = res.u64(root + 0x30)?;
    let natives_at = res.u64(root + 0x40)?;
    let natives = (0..natives_count)
        .map(|i| Ok(res.u64(natives_at + i * 8)?.rotate_left(((code_len as u64 + i) % 64) as u32)))
        .collect::<Result<Vec<_>, ResourceError>>()?;
    let statics = (0..statics_count)
        .map(|i| Ok(res.u64(statics_at + i * 8)? as i64))
        .collect::<Result<Vec<_>, ResourceError>>()?;
    let mut script = Script {
        name: if name_ptr != 0 { res.string(name_ptr)? } else { String::new() },
        name_hash: res.u32(root + 0x58)?,
        code: pages(res, res.u64(root + 0x10)?, code_len)?,
        parameters: res.u32(root + 0x20)?,
        statics,
        natives,
        strings: if strings_len > 0 { pages(res, res.u64(root + 0x68)?, strings_len)? } else { Vec::new() },
        table: None,
    };
    script.table = script.detect_table();
    Ok(script)
}

impl Script {
    /// The string at an offset of the string table.
    pub fn string(&self, offset: usize) -> Option<String> {
        let tail = self.strings.get(offset..)?;
        let end = tail.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    /// All strings of the string table.
    pub fn all_strings(&self) -> Vec<String> {
        self.strings
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }

    /// The opcode table under which the whole code decodes and every jump
    /// lands on an instruction.
    pub fn detect_table(&self) -> Option<Table> {
        [Table::Current, Table::Classic].into_iter().find(|&t| self.check_with(t).is_ok())
    }

    /// Decodes the code with the detected table (or the current one).
    pub fn disassemble(&self) -> Result<Vec<Instruction>, String> {
        self.disassemble_with(self.table.unwrap_or(Table::Current))
    }

    /// Decodes the code. Fails at an unknown opcode or an instruction that runs
    /// past the end (which means the opcode table does not fit).
    pub fn disassemble_with(&self, t: Table) -> Result<Vec<Instruction>, String> {
        let opcodes = table(t);
        let c = &self.code;
        let mut out = Vec::new();
        let mut pc = 0;
        while pc < c.len() {
            let op = c[pc];
            let (name, kind) = *opcodes.get(op as usize).ok_or_else(|| format!("unknown opcode {op} at {pc:#x}"))?;
            let at = |n: usize| -> Result<&[u8], String> {
                c.get(pc + 1..pc + 1 + n).ok_or_else(|| format!("{name} at {pc:#x} runs past the end"))
            };
            let mut targets = Vec::new();
            let mut function = None;
            let (len, operands) = match kind {
                O::None => (0, String::new()),
                O::U8 => (1, at(1)?[0].to_string()),
                O::U8x2 => {
                    let b = at(2)?;
                    (2, format!("{} {}", b[0], b[1]))
                }
                O::U8x3 => {
                    let b = at(3)?;
                    (3, format!("{} {} {}", b[0], b[1], b[2]))
                }
                O::U16 => {
                    let b = at(2)?;
                    (2, u16::from_le_bytes([b[0], b[1]]).to_string())
                }
                O::S16 => {
                    let b = at(2)?;
                    (2, i16::from_le_bytes([b[0], b[1]]).to_string())
                }
                O::U24 => {
                    let b = at(3)?;
                    (3, (b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16).to_string())
                }
                O::U32 => {
                    let b = at(4)?;
                    (4, (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i32).to_string())
                }
                O::F32 => {
                    let b = at(4)?;
                    (4, format!("{}", f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                }
                O::Native => {
                    let b = at(3)?;
                    let (args, rets) = (b[0] >> 2, b[0] & 3);
                    let index = u16::from_be_bytes([b[1], b[2]]) as usize;
                    let hash = self.natives.get(index).copied().unwrap_or_default();
                    (3, format!("0x{hash:016X}  args {args}, returns {rets}"))
                }
                O::Enter => {
                    let b = at(4)?;
                    let params = b[0];
                    let locals = u16::from_le_bytes([b[1], b[2]]);
                    let name_len = b[3] as usize;
                    let name = at(4 + name_len)?[4..].to_vec();
                    let name = String::from_utf8_lossy(name.strip_suffix(&[0]).unwrap_or(&name)).into_owned();
                    if !name.is_empty() {
                        function = Some(name);
                    }
                    (4 + name_len, format!("params {params}, locals {locals}"))
                }
                O::Leave => {
                    let b = at(2)?;
                    (2, format!("params {}, returns {}", b[0], b[1]))
                }
                O::Switch => {
                    let count = at(1)?[0] as usize;
                    let body = at(1 + count * 6)?;
                    let next = pc + 2 + count * 6;
                    let mut cases = Vec::with_capacity(count);
                    for k in 0..count {
                        let e = &body[1 + k * 6..];
                        let value = i32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                        // Each case's jump is relative to the end of that case.
                        let jump = i16::from_le_bytes([e[4], e[5]]) as isize;
                        let target = (pc as isize + 2 + (k as isize + 1) * 6 + jump) as usize;
                        targets.push(target);
                        cases.push(format!("{value}: @{target:#x}"));
                    }
                    let _ = next;
                    (1 + count * 6, cases.join(", "))
                }
                O::Jump => {
                    let b = at(2)?;
                    let target = (pc as isize + 3 + i16::from_le_bytes([b[0], b[1]]) as isize) as usize;
                    targets.push(target);
                    (2, format!("@{target:#x}"))
                }
                O::Call => {
                    let b = at(3)?;
                    let target = b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16;
                    targets.push(target);
                    (3, format!("@{target:#x}"))
                }
            };
            out.push(Instruction { offset: pc, name, operands, targets, function });
            pc += 1 + len;
        }
        Ok(out)
    }

    /// Disassembly as text: functions with their names, labels at jump
    /// targets, strings and native hashes resolved.
    pub fn listing(&self, native_name: impl Fn(u64) -> Option<String>) -> Result<String, String> {
        let code = self.disassemble()?;
        let mut labels: Vec<usize> = code.iter().flat_map(|i| i.targets.iter().copied()).collect();
        labels.sort_unstable();
        labels.dedup();
        let mut out = String::new();
        let mut functions = 0;
        let mut last_push: Option<i64> = None;
        for ins in &code {
            if ins.name == "ENTER" {
                let name = ins.function.clone().unwrap_or_else(|| format!("func_{functions}"));
                out.push_str(&format!("\n{name}:  // @{:#x}\n", ins.offset));
                functions += 1;
            } else if labels.binary_search(&ins.offset).is_ok() {
                out.push_str(&format!("  @{:#x}:\n", ins.offset));
            }
            let mut line = format!("    {:06x}  {:<26}{}", ins.offset, ins.name, ins.operands);
            if ins.name == "STRING"
                && let Some(s) = last_push.and_then(|o| usize::try_from(o).ok()).and_then(|o| self.string(o))
            {
                line.push_str(&format!("  // \"{}\"", s.escape_default()));
            }
            if ins.name == "NATIVE"
                && let Some(hash) =
                    ins.operands.split_whitespace().next().and_then(|h| u64::from_str_radix(&h[2..], 16).ok())
                && let Some(n) = native_name(hash)
            {
                line.push_str(&format!("  // {n}"));
            }
            out.push_str(line.trim_end());
            out.push('\n');
            last_push = match ins.name {
                "PUSH_CONST_U8" | "PUSH_CONST_S16" | "PUSH_CONST_U24" | "PUSH_CONST_U32" => ins.operands.parse().ok(),
                n if n.starts_with("PUSH_CONST_") && n.len() == 12 => n[11..].parse().ok(),
                _ => None,
            };
        }
        Ok(out)
    }

    /// Checks the disassembly: every jump and call lands on an instruction.
    pub fn check(&self) -> Result<usize, String> {
        self.check_with(self.table.unwrap_or(Table::Current))
    }

    fn check_with(&self, t: Table) -> Result<usize, String> {
        let code = self.disassemble_with(t)?;
        let starts: Vec<usize> = code.iter().map(|i| i.offset).collect();
        for ins in &code {
            for t in &ins.targets {
                // Jumping to the very end (after a function's last LEAVE) is fine.
                if *t != self.code.len() && starts.binary_search(t).is_err() {
                    return Err(format!("{} at {:#x} points into an instruction ({t:#x})", ins.name, ins.offset));
                }
            }
        }
        Ok(code.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_small_function() {
        // ENTER 0 params, 2 locals, no name; PUSH_CONST_1; JZ +1; NOP; LEAVE 0 0
        let op = |n: &str| OPCODES.iter().position(|(m, _)| *m == n).unwrap() as u8;
        let mut s = Script {
            code: vec![op("ENTER"), 0, 2, 0, 0, op("PUSH_CONST_1"), op("JZ"), 1, 0, 0, op("LEAVE"), 0, 0],
            ..Default::default()
        };
        s.table = s.detect_table();
        assert_eq!(s.table, Some(Table::Current));
        let code = s.disassemble().unwrap();
        let names: Vec<&str> = code.iter().map(|i| i.name).collect();
        assert_eq!(names, vec!["ENTER", "PUSH_CONST_1", "JZ", "NOP", "LEAVE"]);
        assert_eq!(code[2].targets, vec![10]);
        assert_eq!(s.check().unwrap(), 5);
        let text = s.listing(|_| None).unwrap();
        assert!(text.contains("func_0:"), "{text}");
        assert!(text.contains("@0xa:"), "{text}");
    }
}
